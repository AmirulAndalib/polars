from __future__ import annotations

import json
from datetime import date, datetime, timedelta
from typing import Any, NamedTuple

import numpy as np
import pytest

import polars as pl
from polars.exceptions import PolarsInefficientMapWarning
from polars.testing import assert_frame_equal, assert_series_equal
from tests.unit.conftest import NUMERIC_DTYPES, TEMPORAL_DTYPES

pytestmark = pytest.mark.filterwarnings(
    "ignore::polars.exceptions.PolarsInefficientMapWarning"
)


@pytest.mark.may_fail_auto_streaming  # dtype not set
def test_map_elements_infer_list() -> None:
    df = pl.DataFrame(
        {
            "int": [1, 2],
            "str": ["a", "b"],
            "bool": [True, None],
        }
    )
    assert df.select([pl.all().map_elements(lambda x: [x])]).dtypes == [pl.List] * 3


def test_map_elements_upcast_null_dtype_empty_list() -> None:
    df = pl.DataFrame({"a": [1, 2]})
    out = df.select(
        pl.col("a").map_elements(lambda _: [], return_dtype=pl.List(pl.Int64))
    )
    assert_frame_equal(
        out, pl.DataFrame({"a": [[], []]}, schema={"a": pl.List(pl.Int64)})
    )


def test_map_elements_arithmetic_consistency() -> None:
    df = pl.DataFrame({"A": ["a", "a"], "B": [2, 3]})
    with pytest.warns(PolarsInefficientMapWarning, match="with this one instead"):
        assert df.group_by("A").agg(
            pl.col("B")
            .implode()
            .map_elements(lambda x: x + 1.0, return_dtype=pl.List(pl.Float64))
        )["B"].to_list() == [[3.0, 4.0]]


@pytest.mark.may_fail_auto_streaming  # dtype not set
def test_map_elements_struct() -> None:
    df = pl.DataFrame(
        {
            "A": ["a", "a", None],
            "B": [2, 3, None],
            "C": [True, False, None],
            "D": [12.0, None, None],
            "E": [None, [1], [2, 3]],
        }
    )
    out = df.with_columns(pl.struct(df.columns).alias("struct")).select(
        pl.col("struct").map_elements(lambda x: x["A"]).alias("A_field"),
        pl.col("struct").map_elements(lambda x: x["B"]).alias("B_field"),
        pl.col("struct").map_elements(lambda x: x["C"]).alias("C_field"),
        pl.col("struct").map_elements(lambda x: x["D"]).alias("D_field"),
        pl.col("struct").map_elements(lambda x: x["E"]).alias("E_field"),
    )
    expected = pl.DataFrame(
        {
            "A_field": ["a", "a", None],
            "B_field": [2, 3, None],
            "C_field": [True, False, None],
            "D_field": [12.0, None, None],
            "E_field": [None, [1], [2, 3]],
        }
    )

    assert_frame_equal(out, expected)


def test_map_elements_numpy_int_out() -> None:
    df = pl.DataFrame({"col1": [2, 4, 8, 16]})
    result = df.with_columns(
        pl.col("col1").map_elements(lambda x: np.left_shift(x, 8)).alias("result")
    )
    expected = pl.DataFrame({"col1": [2, 4, 8, 16], "result": [512, 1024, 2048, 4096]})
    assert_frame_equal(result, expected)

    df = pl.DataFrame({"col1": [2, 4, 8, 16], "shift": [1, 1, 2, 2]})
    result = df.select(
        pl.struct(["col1", "shift"])
        .map_elements(lambda cols: np.left_shift(cols["col1"], cols["shift"]))
        .alias("result")
    )
    expected = pl.DataFrame({"result": [4, 8, 32, 64]})
    assert_frame_equal(result, expected)


def test_datelike_identity() -> None:
    for s in [
        pl.Series([datetime(year=2000, month=1, day=1)]),
        pl.Series([timedelta(hours=2)]),
        pl.Series([date(year=2000, month=1, day=1)]),
    ]:
        assert s.map_elements(lambda x: x).to_list() == s.to_list()


def test_map_elements_list_any_value_fallback() -> None:
    with pytest.warns(
        PolarsInefficientMapWarning,
        match=r'(?s)with this one instead:.*pl.col\("text"\).str.json_decode()',
    ):
        df = pl.DataFrame({"text": ['[{"x": 1, "y": 2}, {"x": 3, "y": 4}]']})
        assert df.select(
            pl.col("text").map_elements(
                json.loads,
                return_dtype=pl.List(pl.Struct({"x": pl.Int64, "y": pl.Int64})),
            )
        ).to_dict(as_series=False) == {"text": [[{"x": 1, "y": 2}, {"x": 3, "y": 4}]]}

        # starts with empty list '[]'
        df = pl.DataFrame(
            {
                "text": [
                    "[]",
                    '[{"x": 1, "y": 2}, {"x": 3, "y": 4}]',
                    '[{"x": 1, "y": 2}]',
                ]
            }
        )
        assert df.select(
            pl.col("text").map_elements(
                json.loads,
                return_dtype=pl.List(pl.Struct({"x": pl.Int64, "y": pl.Int64})),
            )
        ).to_dict(as_series=False) == {
            "text": [[], [{"x": 1, "y": 2}, {"x": 3, "y": 4}], [{"x": 1, "y": 2}]]
        }


def test_map_elements_all_types() -> None:
    # test we don't panic
    dtypes = NUMERIC_DTYPES + TEMPORAL_DTYPES + [pl.Decimal(None, 2)]
    for dtype in dtypes:
        pl.Series([1, 2, 3, 4, 5], dtype=dtype).map_elements(lambda x: x)


def test_map_elements_type_propagation() -> None:
    assert (
        pl.from_dict(
            {
                "a": [1, 2, 3],
                "b": [{"c": 1, "d": 2}, {"c": 2, "d": 3}, {"c": None, "d": None}],
            }
        )
        .group_by("a", maintain_order=True)
        .agg(
            [
                pl.when(~pl.col("b").has_nulls())
                .then(
                    pl.col("b")
                    .implode()
                    .map_elements(
                        lambda s: s[0]["c"],
                        return_dtype=pl.Float64,
                    )
                )
                .otherwise(None)
            ]
        )
    ).to_dict(as_series=False) == {"a": [1, 2, 3], "b": [1.0, 2.0, None]}


@pytest.mark.may_fail_auto_streaming  # dtype not set
def test_empty_list_in_map_elements() -> None:
    df = pl.DataFrame(
        {"a": [[1], [1, 2], [3, 4], [5, 6]], "b": [[3], [1, 2], [1, 2], [4, 5]]}
    )

    assert df.select(
        pl.struct(["a", "b"]).map_elements(
            lambda row: list(set(row["a"]) & set(row["b"]))
        )
    ).to_dict(as_series=False) == {"a": [[], [1, 2], [], [5]]}


@pytest.mark.parametrize("value", [1, True, "abc", [1, 2], {"a": 1}])
@pytest.mark.parametrize("return_value", [1, True, "abc", [1, 2], {"a": 1}])
def test_map_elements_skip_nulls(value: Any, return_value: Any) -> None:
    s = pl.Series([value, None])

    result = s.map_elements(lambda x: return_value, skip_nulls=True).to_list()
    assert result == [return_value, None]

    result = s.map_elements(lambda x: return_value, skip_nulls=False).to_list()
    assert result == [return_value, return_value]


def test_map_elements_object_dtypes() -> None:
    with pytest.warns(
        PolarsInefficientMapWarning,
        match=r"(?s)Replace this expression.*lambda x:",
    ):
        assert pl.DataFrame(
            {"a": pl.Series([1, 2, "a", 4, 5], dtype=pl.Object)}
        ).with_columns(
            pl.col("a").map_elements(lambda x: x * 2, return_dtype=pl.Object),
            pl.col("a")
            .map_elements(
                lambda x: isinstance(x, (int, float)), return_dtype=pl.Boolean
            )
            .alias("is_numeric1"),
            pl.col("a")
            .map_elements(
                lambda x: isinstance(x, (int, float)), return_dtype=pl.Boolean
            )
            .alias("is_numeric_infer"),
        ).to_dict(as_series=False) == {
            "a": [2, 4, "aa", 8, 10],
            "is_numeric1": [True, True, False, True, True],
            "is_numeric_infer": [True, True, False, True, True],
        }


def test_map_elements_explicit_list_output_type() -> None:
    out = pl.DataFrame({"str": ["a", "b"]}).with_columns(
        pl.col("str").map_elements(
            lambda _: pl.Series([1, 2, 3]), return_dtype=pl.List(pl.Int64)
        )
    )

    assert out.dtypes == [pl.List(pl.Int64)]
    assert out.to_dict(as_series=False) == {"str": [[1, 2, 3], [1, 2, 3]]}


@pytest.mark.may_fail_auto_streaming  # dtype not set
def test_map_elements_dict() -> None:
    with pytest.warns(
        PolarsInefficientMapWarning,
        match=r'(?s)with this one instead:.*pl.col\("abc"\).str.json_decode()',
    ):
        df = pl.DataFrame({"abc": ['{"A":"Value1"}', '{"B":"Value2"}']})
        assert df.select(
            pl.col("abc").map_elements(
                json.loads, return_dtype=pl.Struct({"A": pl.String, "B": pl.String})
            )
        ).to_dict(as_series=False) == {
            "abc": [{"A": "Value1", "B": None}, {"A": None, "B": "Value2"}]
        }
        assert pl.DataFrame(
            {"abc": ['{"A":"Value1", "B":"Value2"}', '{"B":"Value3"}']}
        ).select(
            pl.col("abc").map_elements(
                json.loads, return_dtype=pl.Struct({"A": pl.String, "B": pl.String})
            )
        ).to_dict(as_series=False) == {
            "abc": [{"A": "Value1", "B": "Value2"}, {"A": None, "B": "Value3"}]
        }


def test_map_elements_pass_name() -> None:
    df = pl.DataFrame(
        {
            "bar": [1, 1, 2],
            "foo": [1, 2, 3],
        }
    )

    mapper = {"foo": "foo1"}

    def element_mapper(s: pl.Series) -> pl.Series:
        return pl.Series([mapper[s.name]])

    assert df.group_by("bar", maintain_order=True).agg(
        pl.col("foo")
        .implode()
        .map_elements(element_mapper, pass_name=True, return_dtype=pl.List(pl.String)),
    ).to_dict(as_series=False) == {"bar": [1, 2], "foo": [["foo1"], ["foo1"]]}


def test_map_elements_binary() -> None:
    assert pl.DataFrame({"bin": [b"\x11" * 12, b"\x22" * 12, b"\xaa" * 12]}).select(
        pl.col("bin").map_elements(bytes.hex)
    ).to_dict(as_series=False) == {
        "bin": [
            "111111111111111111111111",
            "222222222222222222222222",
            "aaaaaaaaaaaaaaaaaaaaaaaa",
        ]
    }


def test_map_elements_set_datetime_output_8984() -> None:
    df = pl.DataFrame({"a": [""]})
    payload = datetime(2001, 1, 1)
    assert df.select(
        pl.col("a").map_elements(lambda _: payload, return_dtype=pl.Datetime),
    )["a"].to_list() == [payload]


def test_map_elements_dict_order_10128() -> None:
    df = pl.select(pl.lit("").map_elements(lambda x: {"c": 1, "b": 2, "a": 3}))
    assert df.to_dict(as_series=False) == {"literal": [{"c": 1, "b": 2, "a": 3}]}


def test_map_elements_10237() -> None:
    df = pl.DataFrame({"a": [1, 2, 3]})
    assert (
        df.select(pl.all().map_elements(lambda x: x > 50))["a"].to_list() == [False] * 3
    )


def test_map_elements_on_empty_col_10639() -> None:
    df = pl.DataFrame({"A": [], "B": []}, schema={"A": pl.Float32, "B": pl.Float32})
    res = df.group_by("B").agg(
        pl.col("A")
        .map_elements(lambda x: x, return_dtype=pl.Int32, strategy="threading")
        .alias("Foo")
    )
    assert res.to_dict(as_series=False) == {
        "B": [],
        "Foo": [],
    }

    res = df.group_by("B").agg(
        pl.col("A")
        .map_elements(lambda x: x, return_dtype=pl.Int32, strategy="thread_local")
        .alias("Foo")
    )
    assert res.to_dict(as_series=False) == {
        "B": [],
        "Foo": [],
    }


def test_map_elements_chunked_14390() -> None:
    s = pl.concat(2 * [pl.Series([1])], rechunk=False)
    assert s.n_chunks() > 1
    with pytest.warns(PolarsInefficientMapWarning):
        assert_series_equal(
            s.map_elements(str, return_dtype=pl.String),
            pl.Series(["1", "1"]),
            check_names=False,
        )


def test_cabbage_strategy_14396() -> None:
    df = pl.DataFrame({"x": [1, 2, 3]})
    with (
        pytest.raises(ValueError, match="strategy 'cabbage' is not supported"),
        pytest.warns(PolarsInefficientMapWarning),
    ):
        df.select(pl.col("x").map_elements(lambda x: 2 * x, strategy="cabbage"))  # type: ignore[arg-type]


def test_map_elements_list_dtype_18472() -> None:
    s = pl.Series([[None], ["abc  ", None]])
    result = s.map_elements(lambda s: [i.strip() if i else None for i in s])
    expected = pl.Series([[None], ["abc", None]])
    assert_series_equal(result, expected)


def test_map_elements_list_return_dtype() -> None:
    s = pl.Series([[1], [2, 3]])
    return_dtype = pl.List(pl.UInt16)

    result = s.map_elements(
        lambda s: [i + 1 for i in s],
        return_dtype=return_dtype,
    )
    expected = pl.Series([[2], [3, 4]], dtype=return_dtype)
    assert_series_equal(result, expected)


def test_map_elements_list_of_named_tuple_15425() -> None:
    class Foo(NamedTuple):
        x: int

    df = pl.DataFrame({"a": [0, 1, 2]})
    result = df.select(
        pl.col("a").map_elements(
            lambda x: [Foo(i) for i in range(x)],
            return_dtype=pl.List(pl.Struct({"x": pl.Int64})),
        )
    )
    expected = pl.DataFrame({"a": [[], [{"x": 0}], [{"x": 0}, {"x": 1}]]})
    assert_frame_equal(result, expected)
