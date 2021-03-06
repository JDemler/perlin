use std::str::FromStr;

use perlin_core::utils::seeking_iterator::{PeekableSeekable};

use language::CanApply;

use query::{Operand, ToOperands};

/// Numberfilter.
/// Takes an string as input and tries to convert it to usize
/// If this is possible it calls the `number_callback` with the resulting usize
/// Otherwise it calls the `string_callback` with the original input
pub struct NumberFilter<TStringCallback, TNumberCallback> {
    string_callback: TStringCallback,
    number_callback: TNumberCallback,
}

impl<TSCB, TNCB> NumberFilter<TSCB, TNCB> {
    pub fn create(number_callback: TNCB, string_callback: TSCB) -> Self {
        NumberFilter {
            string_callback: string_callback,
            number_callback: number_callback,
        }
    }
}

impl<'a, TStringCallback, TNumberCallback> CanApply<&'a str>
    for NumberFilter<TStringCallback, TNumberCallback>
    where TStringCallback: CanApply<&'a str>,
          TNumberCallback: CanApply<u64>
{
    type Output = TStringCallback::Output;
    fn apply(&mut self, input: &'a str) {
        if let Ok(number) = u64::from_str(input) {
            self.number_callback.apply(number);
        } else {
            self.string_callback.apply(input);
        }
    }
}

impl<'a, TStringCallback, TNumberCallback> ToOperands<'a>
    for NumberFilter<TStringCallback, TNumberCallback>
    where TStringCallback: ToOperands<'a>,
          TNumberCallback: ToOperands<'a>
{
    fn to_operands(self) -> Vec<PeekableSeekable<Operand<'a>>> {
        let mut result = self.number_callback.to_operands();
        result.append(&mut self.string_callback.to_operands());
        result
    }
}



pub struct ToUsize<TCallback> {
    callback: TCallback,
}

impl<TCallback> ToUsize<TCallback> {
    pub fn create(callback: TCallback) -> Self {
        ToUsize { callback: callback }
    }
}

impl<'a, TCallback> CanApply<&'a str> for ToUsize<TCallback>
    where TCallback: CanApply<usize>
{
    type Output = TCallback::Output;

    fn apply(&mut self, input: &'a str) {
        if let Ok(number) = usize::from_str(input) {
            self.callback.apply(number);
        }
    }
}
