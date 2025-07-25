use std::fmt;
use std::path::PathBuf;

use polars_core::schema::Schema;
use polars_utils::pl_str::PlSmallStr;
use polars_utils::unique_id::UniqueId;

use super::format::ExprIRSliceDisplay;
use crate::constants::UNLIMITED_CACHE;
use crate::prelude::ir::format::ColumnsDisplay;
use crate::prelude::*;

pub struct IRDotDisplay<'a> {
    lp: IRPlanRef<'a>,
}

const INDENT: &str = "  ";

#[derive(Clone, Copy)]
enum DotNode {
    Plain(usize),
    Cache(UniqueId),
}

impl fmt::Display for DotNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DotNode::Plain(n) => write!(f, "p{n}"),
            DotNode::Cache(n) => write!(f, "c{n}"),
        }
    }
}

#[inline(always)]
fn write_label<'a, 'b>(
    f: &'a mut fmt::Formatter<'b>,
    id: DotNode,
    mut w: impl FnMut(&mut EscapeLabel<'a>) -> fmt::Result,
) -> fmt::Result {
    write!(f, "{INDENT}{id}[label=\"")?;

    let mut escaped = EscapeLabel(f);
    w(&mut escaped)?;
    let EscapeLabel(f) = escaped;

    writeln!(f, "\"]")?;

    Ok(())
}

impl<'a> IRDotDisplay<'a> {
    pub fn new(lp: IRPlanRef<'a>) -> Self {
        Self { lp }
    }

    fn with_root(&self, root: Node) -> Self {
        Self {
            lp: self.lp.with_root(root),
        }
    }

    fn display_expr(&self, expr: &'a ExprIR) -> ExprIRDisplay<'a> {
        expr.display(self.lp.expr_arena)
    }

    fn display_exprs(&self, exprs: &'a [ExprIR]) -> ExprIRSliceDisplay<'a, ExprIR> {
        ExprIRSliceDisplay {
            exprs,
            expr_arena: self.lp.expr_arena,
        }
    }

    fn _format(
        &self,
        f: &mut fmt::Formatter<'_>,
        parent: Option<DotNode>,
        last: &mut usize,
    ) -> std::fmt::Result {
        use fmt::Write;

        let root = self.lp.root();
        let id = if let IR::Cache { id, .. } = root {
            DotNode::Cache(*id)
        } else {
            *last += 1;
            DotNode::Plain(*last)
        };

        if let Some(parent) = parent {
            writeln!(f, "{INDENT}{parent} -- {id}")?;
        }

        use IR::*;
        match root {
            Union { inputs, .. } => {
                for input in inputs {
                    self.with_root(*input)._format(f, Some(id), last)?;
                }

                write_label(f, id, |f| f.write_str("UNION"))?;
            },
            HConcat { inputs, .. } => {
                for input in inputs {
                    self.with_root(*input)._format(f, Some(id), last)?;
                }

                write_label(f, id, |f| f.write_str("HCONCAT"))?;
            },
            Cache {
                input, cache_hits, ..
            } => {
                self.with_root(*input)._format(f, Some(id), last)?;

                if *cache_hits == UNLIMITED_CACHE {
                    write_label(f, id, |f| f.write_str("CACHE"))?;
                } else {
                    write_label(f, id, |f| write!(f, "CACHE: {cache_hits} times"))?;
                };
            },
            Filter { predicate, input } => {
                self.with_root(*input)._format(f, Some(id), last)?;

                let pred = self.display_expr(predicate);
                write_label(f, id, |f| write!(f, "FILTER BY {pred}"))?;
            },
            #[cfg(feature = "python")]
            PythonScan { options } => {
                let predicate = match &options.predicate {
                    PythonPredicate::Polars(e) => format!("{}", self.display_expr(e)),
                    PythonPredicate::PyArrow(s) => s.clone(),
                    PythonPredicate::None => "none".to_string(),
                };
                let with_columns = NumColumns(options.with_columns.as_ref().map(|s| s.as_ref()));
                let total_columns = options.schema.len();

                write_label(f, id, |f| {
                    write!(
                        f,
                        "PYTHON SCAN\nπ {with_columns}/{total_columns};\nσ {predicate}"
                    )
                })?
            },
            Select {
                expr,
                input,
                schema,
                ..
            } => {
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| write!(f, "π {}/{}", expr.len(), schema.len()))?;
            },
            Sort {
                input, by_column, ..
            } => {
                let by_column = self.display_exprs(by_column);
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| write!(f, "SORT BY {by_column}"))?;
            },
            GroupBy {
                input, keys, aggs, ..
            } => {
                let keys = self.display_exprs(keys);
                let aggs = self.display_exprs(aggs);
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| write!(f, "AGG {aggs}\nBY\n{keys}"))?;
            },
            HStack { input, exprs, .. } => {
                let exprs = self.display_exprs(exprs);
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| write!(f, "WITH COLUMNS {exprs}"))?;
            },
            Slice { input, offset, len } => {
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| write!(f, "SLICE offset: {offset}; len: {len}"))?;
            },
            Distinct { input, options, .. } => {
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| {
                    f.write_str("DISTINCT")?;

                    if let Some(subset) = &options.subset {
                        f.write_str(" BY ")?;

                        let mut subset = subset.iter();

                        if let Some(fst) = subset.next() {
                            f.write_str(fst)?;
                            for name in subset {
                                write!(f, ", \"{name}\"")?;
                            }
                        } else {
                            f.write_str("None")?;
                        }
                    }

                    Ok(())
                })?;
            },
            DataFrameScan {
                schema,
                output_schema,
                ..
            } => {
                let num_columns = NumColumnsSchema(output_schema.as_ref().map(|p| p.as_ref()));
                let total_columns = schema.len();

                write_label(f, id, |f| {
                    write!(f, "TABLE\nπ {num_columns}/{total_columns}")
                })?;
            },
            Scan {
                sources,
                file_info,
                hive_parts: _,
                predicate,
                scan_type,
                unified_scan_args,
                output_schema: _,
            } => {
                let name: &str = (&**scan_type).into();
                let path = ScanSourcesDisplay(sources);
                let with_columns = unified_scan_args
                    .projection
                    .as_ref()
                    .map(|cols| cols.as_ref());
                let with_columns = NumColumns(with_columns);
                let total_columns =
                    file_info.schema.len() - usize::from(unified_scan_args.row_index.is_some());

                write_label(f, id, |f| {
                    write!(f, "{name} SCAN {path}\nπ {with_columns}/{total_columns};",)?;

                    if let Some(predicate) = predicate.as_ref() {
                        write!(f, "\nσ {}", self.display_expr(predicate))?;
                    }

                    if let Some(row_index) = unified_scan_args.row_index.as_ref() {
                        write!(f, "\nrow index: {} (+{})", row_index.name, row_index.offset)?;
                    }

                    Ok(())
                })?;
            },
            Join {
                input_left,
                input_right,
                left_on,
                right_on,
                options,
                ..
            } => {
                self.with_root(*input_left)._format(f, Some(id), last)?;
                self.with_root(*input_right)._format(f, Some(id), last)?;

                write_label(f, id, |f| {
                    write!(f, "JOIN {}", options.args.how)?;

                    if !left_on.is_empty() {
                        let left_on = self.display_exprs(left_on);
                        let right_on = self.display_exprs(right_on);
                        write!(f, "\nleft: {left_on};\nright: {right_on}")?
                    }
                    Ok(())
                })?;
            },
            MapFunction {
                input, function, ..
            } => {
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| write!(f, "{function}"))?;
            },
            ExtContext { input, .. } => {
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| f.write_str("EXTERNAL_CONTEXT"))?;
            },
            Sink { input, payload, .. } => {
                self.with_root(*input)._format(f, Some(id), last)?;

                write_label(f, id, |f| {
                    f.write_str(match payload {
                        SinkTypeIR::Memory => "SINK (MEMORY)",
                        SinkTypeIR::File { .. } => "SINK (FILE)",
                        SinkTypeIR::Partition { .. } => "SINK (PARTITION)",
                    })
                })?;
            },
            SinkMultiple { inputs } => {
                for input in inputs {
                    self.with_root(*input)._format(f, Some(id), last)?;
                }

                write_label(f, id, |f| f.write_str("SINK MULTIPLE"))?;
            },
            SimpleProjection { input, columns } => {
                let num_columns = columns.as_ref().len();
                let total_columns = self.lp.lp_arena.get(*input).schema(self.lp.lp_arena).len();

                let columns = ColumnsDisplay(columns.as_ref());
                self.with_root(*input)._format(f, Some(id), last)?;
                write_label(f, id, |f| {
                    write!(f, "simple π {num_columns}/{total_columns}\n[{columns}]")
                })?;
            },
            #[cfg(feature = "merge_sorted")]
            MergeSorted {
                input_left,
                input_right,
                key,
            } => {
                self.with_root(*input_left)._format(f, Some(id), last)?;
                self.with_root(*input_right)._format(f, Some(id), last)?;

                write_label(f, id, |f| write!(f, "MERGE_SORTED ON '{key}'",))?;
            },
            Invalid => write_label(f, id, |f| f.write_str("INVALID"))?,
        }

        Ok(())
    }
}

// A few utility structures for formatting
pub struct PathsDisplay<'a>(pub &'a [PathBuf]);
pub struct ScanSourcesDisplay<'a>(pub &'a ScanSources);
struct NumColumns<'a>(Option<&'a [PlSmallStr]>);
struct NumColumnsSchema<'a>(Option<&'a Schema>);

impl fmt::Display for ScanSourceRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanSourceRef::Path(addr) => addr.display().fmt(f),
            ScanSourceRef::File(_) => f.write_str("open-file"),
            ScanSourceRef::Buffer(buff) => write!(f, "{} in-mem bytes", buff.len()),
        }
    }
}

impl fmt::Display for ScanSourcesDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.len() {
            0 => write!(f, "[]"),
            1 => write!(f, "[{}]", self.0.at(0)),
            2 => write!(f, "[{}, {}]", self.0.at(0), self.0.at(1)),
            _ => write!(
                f,
                "[{}, ... {} other sources]",
                self.0.at(0),
                self.0.len() - 1,
            ),
        }
    }
}

impl fmt::Display for PathsDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.len() {
            0 => write!(f, "[]"),
            1 => write!(f, "[{}]", self.0[0].display()),
            2 => write!(f, "[{}, {}]", self.0[0].display(), self.0[1].display()),
            _ => write!(
                f,
                "[{}, ... {} other files]",
                self.0[0].display(),
                self.0.len() - 1,
            ),
        }
    }
}

impl fmt::Display for NumColumns<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("*"),
            Some(columns) => columns.len().fmt(f),
        }
    }
}

impl fmt::Display for NumColumnsSchema<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("*"),
            Some(columns) => columns.len().fmt(f),
        }
    }
}

/// Utility structure to write to a [`fmt::Formatter`] whilst escaping the output as a label name
pub struct EscapeLabel<'a>(pub &'a mut dyn fmt::Write);

impl fmt::Write for EscapeLabel<'_> {
    fn write_str(&mut self, mut s: &str) -> fmt::Result {
        loop {
            let mut char_indices = s.char_indices();

            // This escapes quotes and new lines
            // @NOTE: I am aware this does not work for \" and such. I am ignoring that fact as we
            // are not really using such strings.
            let f = char_indices.find_map(|(i, c)| match c {
                '"' => Some((i, r#"\""#)),
                '\n' => Some((i, r#"\n"#)),
                _ => None,
            });

            let Some((at, to_write)) = f else {
                break;
            };

            self.0.write_str(&s[..at])?;
            self.0.write_str(to_write)?;
            s = &s[at + 1..];
        }

        self.0.write_str(s)?;

        Ok(())
    }
}

impl fmt::Display for IRDotDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "graph  polars_query {{")?;

        let mut last = 0;
        self._format(f, None, &mut last)?;

        writeln!(f, "}}")?;

        Ok(())
    }
}
