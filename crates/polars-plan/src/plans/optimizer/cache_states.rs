use std::collections::BTreeMap;

use polars_utils::unique_id::UniqueId;

use super::*;

fn get_upper_projections(
    parent: Node,
    lp_arena: &Arena<IR>,
    expr_arena: &Arena<AExpr>,
    names_scratch: &mut Vec<PlSmallStr>,
    found_required_columns: &mut bool,
) -> bool {
    let parent = lp_arena.get(parent);

    use IR::*;
    // During projection pushdown all accumulated.
    match parent {
        SimpleProjection { columns, .. } => {
            let iter = columns.iter_names_cloned();
            names_scratch.extend(iter);
            *found_required_columns = true;
            false
        },
        Filter { predicate, .. } => {
            // Also add predicate, as the projection is above the filter node.
            names_scratch.extend(aexpr_to_leaf_names(predicate.node(), expr_arena));

            true
        },
        // Only filter and projection nodes are allowed, any other node we stop.
        _ => false,
    }
}

fn get_upper_predicates(
    parent: Node,
    lp_arena: &Arena<IR>,
    expr_arena: &mut Arena<AExpr>,
    predicate_scratch: &mut Vec<Expr>,
) -> bool {
    let parent = lp_arena.get(parent);

    use IR::*;
    match parent {
        Filter { predicate, .. } => {
            let expr = predicate.to_expr(expr_arena);
            predicate_scratch.push(expr);
            false
        },
        SimpleProjection { .. } => true,
        // Only filter and projection nodes are allowed, any other node we stop.
        _ => false,
    }
}

type TwoParents = [Option<Node>; 2];

// 1. This will ensure that all equal caches communicate the amount of columns
//    they need to project.
// 2. This will ensure we apply predicate in the subtrees below the caches.
//    If the predicate above the cache is the same for all matching caches, that filter will be
//    applied as well.
//
// # Example
// Consider this tree, where `SUB-TREE` is duplicate and can be cached.
//
//
//                         Tree
//                         |
//                         |
//    |--------------------|-------------------|
//    |                                        |
//    SUB-TREE                                 SUB-TREE
//
// STEPS:
// - 1. CSE will run and will insert cache nodes
//
//                         Tree
//                         |
//                         |
//    |--------------------|-------------------|
//    |                                        |
//    | CACHE 0                                | CACHE 0
//    |                                        |
//    SUB-TREE                                 SUB-TREE
//
// - 2. predicate and projection pushdown will run and will insert optional FILTER and PROJECTION above the caches
//
//                         Tree
//                         |
//                         |
//    |--------------------|-------------------|
//    | FILTER (optional)                      | FILTER (optional)
//    | PROJ (optional)                        | PROJ (optional)
//    |                                        |
//    | CACHE 0                                | CACHE 0
//    |                                        |
//    SUB-TREE                                 SUB-TREE
//
// # Projection optimization
// The union of the projection is determined and the projection will be pushed down.
//
//                         Tree
//                         |
//                         |
//    |--------------------|-------------------|
//    | FILTER (optional)                      | FILTER (optional)
//    | CACHE 0                                | CACHE 0
//    |                                        |
//    SUB-TREE                                 SUB-TREE
//    UNION PROJ (optional)                    UNION PROJ (optional)
//
// # Filter optimization
// Depending on the predicates the predicate pushdown optimization will run.
// Possible cases:
// - NO FILTERS: run predicate pd from the cache nodes -> finish
// - Above the filters the caches are the same -> run predicate pd from the filter node -> finish
// - There is a cache without predicates above the cache node -> run predicate form the cache nodes -> finish
// - The predicates above the cache nodes are all different -> remove the cache nodes -> finish
#[expect(clippy::too_many_arguments)]
pub(super) fn set_cache_states(
    root: Node,
    lp_arena: &mut Arena<IR>,
    expr_arena: &mut Arena<AExpr>,
    scratch: &mut Vec<Node>,
    expr_eval: ExprEval<'_>,
    verbose: bool,
    pushdown_maintain_errors: bool,
    new_streaming: bool,
) -> PolarsResult<()> {
    let mut stack = Vec::with_capacity(4);
    let mut names_scratch = vec![];
    let mut predicates_scratch = vec![];

    scratch.clear();
    stack.clear();

    #[derive(Default)]
    struct Value {
        // All the children of the cache per cache-id.
        children: Vec<Node>,
        parents: Vec<TwoParents>,
        cache_nodes: Vec<Node>,
        // Union over projected names.
        names_union: PlHashSet<PlSmallStr>,
        // Union over predicates.
        predicate_union: PlHashMap<Expr, u32>,
    }
    let mut cache_schema_and_children = BTreeMap::new();

    // Stack frame
    #[derive(Default, Clone)]
    struct Frame {
        current: Node,
        cache_id: Option<UniqueId>,
        parent: TwoParents,
        previous_cache: Option<UniqueId>,
    }
    let init = Frame {
        current: root,
        ..Default::default()
    };

    stack.push(init);

    // # First traversal.
    // Collect the union of columns per cache id.
    // And find the cache parents.
    while let Some(mut frame) = stack.pop() {
        let lp = lp_arena.get(frame.current);
        lp.copy_inputs(scratch);

        use IR::*;

        if let Cache { input, id, .. } = lp {
            if let Some(cache_id) = frame.cache_id {
                frame.previous_cache = Some(cache_id)
            }
            if frame.parent[0].is_some() {
                // Projection pushdown has already run and blocked on cache nodes
                // the pushed down columns are projected just above this cache
                // if there were no pushed down column, we just take the current
                // nodes schema
                // we never want to naively take parents, as a join or aggregate for instance
                // change the schema

                let v = cache_schema_and_children
                    .entry(*id)
                    .or_insert_with(Value::default);
                v.children.push(*input);
                v.parents.push(frame.parent);
                v.cache_nodes.push(frame.current);

                let mut found_required_columns = false;

                for parent_node in frame.parent.into_iter().flatten() {
                    let keep_going = get_upper_projections(
                        parent_node,
                        lp_arena,
                        expr_arena,
                        &mut names_scratch,
                        &mut found_required_columns,
                    );
                    if !names_scratch.is_empty() {
                        v.names_union.extend(names_scratch.drain(..));
                    }
                    // We stop early as we want to find the first projection node above the cache.
                    if !keep_going {
                        break;
                    }
                }

                for parent_node in frame.parent.into_iter().flatten() {
                    let keep_going = get_upper_predicates(
                        parent_node,
                        lp_arena,
                        expr_arena,
                        &mut predicates_scratch,
                    );
                    if !predicates_scratch.is_empty() {
                        for pred in predicates_scratch.drain(..) {
                            let count = v.predicate_union.entry(pred).or_insert(0);
                            *count += 1;
                        }
                    }
                    // We stop early as we want to find the first predicate node above the cache.
                    if !keep_going {
                        break;
                    }
                }

                // There was no explicit projection and we must take
                // all columns
                if !found_required_columns {
                    let schema = lp.schema(lp_arena);
                    v.names_union.extend(schema.iter_names_cloned());
                }
            }
            frame.cache_id = Some(*id);
        };

        // Shift parents.
        frame.parent[1] = frame.parent[0];
        frame.parent[0] = Some(frame.current);
        for n in scratch.iter() {
            let mut new_frame = frame.clone();
            new_frame.current = *n;
            stack.push(new_frame);
        }
        scratch.clear();
    }

    // # Second pass.
    // we create a subtree where we project the columns
    // just before the cache. Then we do another projection pushdown
    // and finally remove that last projection and stitch the subplan
    // back to the cache node again
    if !cache_schema_and_children.is_empty() {
        let mut proj_pd = ProjectionPushDown::new();
        let mut pred_pd =
            PredicatePushDown::new(expr_eval, pushdown_maintain_errors, new_streaming)
                .block_at_cache(false);
        for (_cache_id, v) in cache_schema_and_children {
            // # CHECK IF WE NEED TO REMOVE CACHES
            // If we encounter multiple predicates we remove the cache nodes completely as we don't
            // want to loose predicate pushdown in favor of scan sharing.
            if v.predicate_union.len() > 1 {
                if verbose {
                    eprintln!("cache nodes will be removed because predicates don't match")
                }
                for ((&child, cache), parents) in
                    v.children.iter().zip(v.cache_nodes).zip(v.parents)
                {
                    // Remove the cache and assign the child the cache location.
                    lp_arena.swap(child, cache);

                    // Restart predicate and projection pushdown from most top parent.
                    // This to ensure we continue the optimization where it was blocked initially.
                    // We pick up the blocked filter and projection.
                    let mut node = cache;
                    for p_node in parents.into_iter().flatten() {
                        if matches!(
                            lp_arena.get(p_node),
                            IR::Filter { .. } | IR::SimpleProjection { .. }
                        ) {
                            node = p_node
                        } else {
                            break;
                        }
                    }

                    let lp = lp_arena.take(node);
                    let lp = proj_pd.optimize(lp, lp_arena, expr_arena)?;
                    let lp = pred_pd.optimize(lp, lp_arena, expr_arena)?;
                    lp_arena.replace(node, lp);
                }
                return Ok(());
            }
            // Below we restart projection and predicates pushdown
            // on the first cache node. As it are cache nodes, the others are the same
            // and we can reuse the optimized state for all inputs.
            // See #21637

            // # RUN PROJECTION PUSHDOWN
            if !v.names_union.is_empty() {
                let first_child = *v.children.first().expect("at least on child");

                let columns = &v.names_union;
                let child_lp = lp_arena.take(first_child);

                // Make sure we project in the order of the schema
                // if we don't a union may fail as we would project by the
                // order we discovered all values.
                let child_schema = child_lp.schema(lp_arena);
                let child_schema = child_schema.as_ref();
                let projection = child_schema
                    .iter_names()
                    .flat_map(|name| columns.get(name.as_str()).cloned())
                    .collect::<Vec<_>>();

                let new_child = lp_arena.add(child_lp);

                let lp = IRBuilder::new(new_child, expr_arena, lp_arena)
                    .project_simple(projection)
                    .expect("unique names")
                    .build();

                let lp = proj_pd.optimize(lp, lp_arena, expr_arena)?;
                // Optimization can lead to a double projection. Only take the last.
                let lp = if let IR::SimpleProjection { input, columns } = lp {
                    let input =
                        if let IR::SimpleProjection { input: input2, .. } = lp_arena.get(input) {
                            *input2
                        } else {
                            input
                        };
                    IR::SimpleProjection { input, columns }
                } else {
                    lp
                };
                lp_arena.replace(first_child, lp.clone());

                // Set the remaining children to the same node.
                for &child in &v.children[1..] {
                    lp_arena.replace(child, lp.clone());
                }
            } else {
                // No upper projections to include, run projection pushdown from cache node.
                let first_child = *v.children.first().expect("at least on child");
                let child_lp = lp_arena.take(first_child);
                let lp = proj_pd.optimize(child_lp, lp_arena, expr_arena)?;
                lp_arena.replace(first_child, lp.clone());

                for &child in &v.children[1..] {
                    lp_arena.replace(child, lp.clone());
                }
            }

            // # RUN PREDICATE PUSHDOWN
            // Run this after projection pushdown, otherwise the predicate columns will not be projected.

            // - If all predicates of parent are the same we will restart predicate pushdown from the parent FILTER node.
            // - Otherwise we will start predicate pushdown from the cache node.
            let allow_parent_predicate_pushdown = v.predicate_union.len() == 1 && {
                let (_pred, count) = v.predicate_union.iter().next().unwrap();
                *count == v.children.len() as u32
            };

            if allow_parent_predicate_pushdown {
                let parents = *v.parents.first().unwrap();
                let node = get_filter_node(parents, lp_arena)
                    .expect("expected filter; this is an optimizer bug");
                let start_lp = lp_arena.take(node);
                let lp = pred_pd.optimize(start_lp, lp_arena, expr_arena)?;
                lp_arena.replace(node, lp.clone());
                for &parents in &v.parents[1..] {
                    let node = get_filter_node(parents, lp_arena)
                        .expect("expected filter; this is an optimizer bug");
                    lp_arena.replace(node, lp.clone());
                }
            } else {
                let child = *v.children.first().unwrap();
                let child_lp = lp_arena.take(child);
                let lp = pred_pd.optimize(child_lp, lp_arena, expr_arena)?;
                lp_arena.replace(child, lp.clone());
                for &child in &v.children[1..] {
                    lp_arena.replace(child, lp.clone());
                }
            }
        }
    }
    Ok(())
}

fn get_filter_node(parents: TwoParents, lp_arena: &Arena<IR>) -> Option<Node> {
    parents
        .into_iter()
        .flatten()
        .find(|&parent| matches!(lp_arena.get(parent), IR::Filter { .. }))
}
