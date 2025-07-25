use polars_core::prelude::*;
use polars_core::with_match_physical_integer_polars_type;
use polars_ops::series::new_int_range;

use super::utils::{ensure_range_bounds_contain_exactly_one_value, numeric_ranges_impl_broadcast};

const CAPACITY_FACTOR: usize = 5;

pub(super) fn int_range(s: &[Column], step: i64, dtype: DataType) -> PolarsResult<Column> {
    let start = &s[0];
    let end = &s[1];
    let name = start.name();

    ensure_range_bounds_contain_exactly_one_value(start, end)?;

    // Done by type coercion
    assert!(dtype.is_integer());
    assert_eq!(start.dtype(), &dtype);
    assert_eq!(end.dtype(), &dtype);

    with_match_physical_integer_polars_type!(dtype, |$T| {
        let start_v = get_first_series_value::<$T>(start)?;
        let end_v = get_first_series_value::<$T>(end)?;
        new_int_range::<$T>(start_v, end_v, step, name.clone()).map(Column::from)
    })
}

fn get_first_series_value<T>(s: &Column) -> PolarsResult<T::Native>
where
    T: PolarsIntegerType,
{
    let ca: &ChunkedArray<T> = s.as_materialized_series().as_any().downcast_ref().unwrap();
    let value_opt = ca.get(0);
    let value =
        value_opt.ok_or_else(|| polars_err!(ComputeError: "invalid null input for `int_range`"))?;
    Ok(value)
}

pub(super) fn int_ranges(s: &[Column], dtype: DataType) -> PolarsResult<Column> {
    let start = &s[0];
    let end = &s[1];
    let step = &s[2];

    let start = start.i64()?;
    let end = end.i64()?;
    let step = step.i64()?;

    let len = std::cmp::max(start.len(), end.len());
    let mut builder = ListPrimitiveChunkedBuilder::<Int64Type>::new(
        // The name should follow our left hand rule.
        start.name().clone(),
        len,
        len * CAPACITY_FACTOR,
        DataType::Int64,
    );

    let range_impl =
        |start, end, step: i64, builder: &mut ListPrimitiveChunkedBuilder<Int64Type>| {
            match step {
                1 => builder.append_values_iter_trusted_len(start..end),
                2.. => builder.append_values_iter_trusted_len((start..end).step_by(step as usize)),
                _ => builder.append_values_iter_trusted_len(
                    (end..start)
                        .step_by(step.unsigned_abs() as usize)
                        .map(|x| start - (x - end)),
                ),
            };
            Ok(())
        };

    let column = numeric_ranges_impl_broadcast(start, end, step, range_impl, &mut builder)?;

    if dtype != DataType::Int64 {
        column.cast(&DataType::List(Box::new(dtype)))
    } else {
        Ok(column)
    }
}
