use num_traits::{FromPrimitive, ToPrimitive};

use super::no_nulls::RollingAggWindowNoNulls;
use super::nulls::RollingAggWindowNulls;
use super::*;
use crate::moment::{KurtosisState, SkewState, VarState};

pub trait StateUpdate {
    fn new(params: Option<RollingFnParams>) -> Self;
    fn reset(&mut self);
    fn insert_one(&mut self, x: f64);
    fn remove_one(&mut self, x: f64);
    fn finalize(&self) -> Option<f64>;
}

pub struct VarianceMoment {
    state: VarState,
    ddof: u8,
}

impl StateUpdate for VarianceMoment {
    fn new(params: Option<RollingFnParams>) -> Self {
        let ddof = if let Some(RollingFnParams::Var(params)) = params {
            params.ddof
        } else {
            1
        };

        Self {
            state: VarState::default(),
            ddof,
        }
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.state = VarState::default();
    }

    #[inline(always)]
    fn insert_one(&mut self, x: f64) {
        self.state.insert_one(x);
    }

    #[inline(always)]
    fn remove_one(&mut self, x: f64) {
        self.state.remove_one(x);
    }

    #[inline(always)]
    fn finalize(&self) -> Option<f64> {
        self.state.finalize(self.ddof)
    }
}

pub struct KurtosisMoment {
    state: KurtosisState,
    fisher: bool,
    bias: bool,
}

impl StateUpdate for KurtosisMoment {
    fn new(params: Option<RollingFnParams>) -> Self {
        let (fisher, bias) = if let Some(RollingFnParams::Kurtosis { fisher, bias }) = params {
            (fisher, bias)
        } else {
            (false, false)
        };

        Self {
            state: KurtosisState::default(),
            fisher,
            bias,
        }
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.state = KurtosisState::default();
    }

    #[inline(always)]
    fn insert_one(&mut self, x: f64) {
        self.state.insert_one(x);
    }

    #[inline(always)]
    fn remove_one(&mut self, x: f64) {
        self.state.remove_one(x);
    }

    #[inline(always)]
    fn finalize(&self) -> Option<f64> {
        self.state.finalize(self.fisher, self.bias)
    }
}

pub struct SkewMoment {
    state: SkewState,
    bias: bool,
}

impl StateUpdate for SkewMoment {
    fn new(params: Option<RollingFnParams>) -> Self {
        let bias = if let Some(RollingFnParams::Skew { bias }) = params {
            bias
        } else {
            false
        };

        Self {
            state: SkewState::default(),
            bias,
        }
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.state = SkewState::default();
    }

    #[inline(always)]
    fn insert_one(&mut self, x: f64) {
        self.state.insert_one(x);
    }

    #[inline(always)]
    fn remove_one(&mut self, x: f64) {
        self.state.remove_one(x);
    }

    #[inline(always)]
    fn finalize(&self) -> Option<f64> {
        self.state.finalize(self.bias)
    }
}

pub struct MomentWindow<'a, T, M: StateUpdate> {
    slice: &'a [T],
    validity: Option<&'a Bitmap>,
    moment: M,
    non_finite_count: usize, // NaN or infinity.
    null_count: usize,
    last_start: usize,
    last_end: usize,
}

impl<'a, T, M> MomentWindow<'a, T, M>
where
    T: NativeType + ToPrimitive + IsFloat + FromPrimitive,
    M: StateUpdate,
{
    fn new_impl(
        slice: &'a [T],
        validity: Option<&'a Bitmap>,
        params: Option<RollingFnParams>,
    ) -> Self {
        Self {
            slice,
            validity,
            moment: M::new(params),
            non_finite_count: 0,
            null_count: 0,
            last_start: 0,
            last_end: 0,
        }
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.moment.reset();
        self.non_finite_count = 0;
        self.null_count = 0;
    }

    #[inline(always)]
    fn insert(&mut self, val: T) {
        if val.is_finite() {
            self.moment.insert_one(NumCast::from(val).unwrap());
        } else {
            self.moment.insert_one(0.0); // A hack to replicate ddof null behavior.
            self.non_finite_count += 1;
        }
    }

    #[inline(always)]
    fn remove(&mut self, val: T) {
        if val.is_finite() {
            self.moment.remove_one(NumCast::from(val).unwrap());
        } else {
            self.moment.remove_one(0.0); // A hack to replicate ddof null behavior.
            self.non_finite_count -= 1;
        }
    }

    #[inline(always)]
    fn finalize(&self) -> Option<T> {
        if self.non_finite_count > 0 {
            self.moment
                .finalize()
                .map(|_v| T::from_f64(f64::NAN).unwrap())
        } else {
            self.moment.finalize().map(|v| T::from_f64(v).unwrap())
        }
    }
}

impl<'a, T, M> RollingAggWindowNoNulls<'a, T> for MomentWindow<'a, T, M>
where
    T: NativeType + ToPrimitive + IsFloat + FromPrimitive,
    M: StateUpdate,
{
    fn new(
        slice: &'a [T],
        start: usize,
        end: usize,
        params: Option<RollingFnParams>,
        _window_size: Option<usize>,
    ) -> Self {
        let mut out = Self::new_impl(slice, None, params);
        unsafe { RollingAggWindowNoNulls::update(&mut out, start, end) };
        out
    }

    // # Safety
    // The start, end range must be in-bounds.
    #[inline]
    unsafe fn update(&mut self, start: usize, end: usize) -> Option<T> {
        if start >= self.last_end {
            self.reset();
            self.last_start = start;
            self.last_end = start;
        }

        for val in &self.slice[self.last_start..start] {
            self.remove(*val);
        }

        for val in &self.slice[self.last_end..end] {
            self.insert(*val);
        }

        self.last_start = start;
        self.last_end = end;
        self.finalize()
    }
}

impl<'a, T, M> RollingAggWindowNulls<'a, T> for MomentWindow<'a, T, M>
where
    T: NativeType + ToPrimitive + IsFloat + FromPrimitive,
    M: StateUpdate,
{
    unsafe fn new(
        slice: &'a [T],
        validity: &'a Bitmap,
        start: usize,
        end: usize,
        params: Option<RollingFnParams>,
        _window_size: Option<usize>,
    ) -> Self {
        let mut out = Self::new_impl(slice, Some(validity), params);
        unsafe { RollingAggWindowNulls::update(&mut out, start, end) };
        out
    }

    // # Safety
    // The start, end range must be in-bounds.
    #[inline]
    unsafe fn update(&mut self, start: usize, end: usize) -> Option<T> {
        let validity = unsafe { self.validity.unwrap_unchecked() };

        if start >= self.last_end {
            self.reset();
            self.last_start = start;
            self.last_end = start;
        }

        for idx in self.last_start..start {
            let valid = unsafe { validity.get_bit_unchecked(idx) };
            if valid {
                self.remove(unsafe { *self.slice.get_unchecked(idx) });
            } else {
                self.null_count -= 1;
            }
        }

        for idx in self.last_end..end {
            let valid = unsafe { validity.get_bit_unchecked(idx) };
            if valid {
                self.insert(unsafe { *self.slice.get_unchecked(idx) });
            } else {
                self.null_count += 1;
            }
        }

        self.last_start = start;
        self.last_end = end;
        self.finalize()
    }

    #[inline(always)]
    fn is_valid(&self, min_periods: usize) -> bool {
        ((self.last_end - self.last_start) - self.null_count) >= min_periods
    }
}
