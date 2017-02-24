use std::fmt::Debug;

use perlin_core::index::posting::DocId;
use query::{ChainedOperand, ToOperands};

mod stemmers;
pub mod integers;
mod stopword_filter;

pub use language::stopword_filter::StopwordFilter;
pub use language::stemmers::Stemmer;

/// The single central trait of the push-based splittable pipeline!
/// Any element in it can be called passing a typed and generic input and a common value
pub trait CanApply<Input> {
    type Output;
    fn apply(&mut self, Input);
}

pub struct AlphaNumericTokenizer<TCB> {
    cb: TCB
}
impl<TCB> AlphaNumericTokenizer<TCB> {
    pub fn create(cb: TCB) -> Self {
        AlphaNumericTokenizer{
            cb: cb
        }
    }
}
impl<'a, TCB> CanApply<&'a str> for AlphaNumericTokenizer<TCB>
    where TCB: CanApply<&'a str>
{
    type Output = TCB::Output;
    fn apply(&mut self, input: &'a str) {
        for token in input.split(|c: char| !c.is_alphanumeric()) {
            if !token.is_empty() {
                self.cb.apply(token);
            }
        }
    }
}

impl<'a, TCB> ToOperands<'a> for AlphaNumericTokenizer<TCB>
    where TCB: ToOperands<'a>
{
    fn to_operands(self) -> Vec<ChainedOperand<'a>> {
        self.cb.to_operands()
    }
}


pub struct Debugger<TCallback>
{
    callback: TCallback,
}
impl<TCallback> Debugger<TCallback> {
    pub fn create(callback: TCallback) -> Self {
        Debugger{
            callback: callback
        }
    }
}

impl<TCallback, T: Debug> CanApply<T> for Debugger<TCallback>
    where TCallback: CanApply<T>
{
    type Output = TCallback::Output;

    fn apply(&mut self, input: T) {
        println!("{:?}", input);
        self.callback.apply(input);
    }
}

impl<'a, TCB> ToOperands<'a> for Debugger<TCB>
    where TCB: ToOperands<'a>
{
    fn to_operands(self) -> Vec<ChainedOperand<'a>>
    {
        self.callback.to_operands()
    }
}



pub struct WhitespaceTokenizer<TCallback>    
{
    callback: TCallback,
}

impl<TCallback> WhitespaceTokenizer<TCallback> {
    pub fn create(callback: TCallback) -> Self {
        WhitespaceTokenizer {
            callback: callback
        }
    }
}

impl<'a, TCallback> CanApply<&'a str> for WhitespaceTokenizer<TCallback>
    where TCallback: CanApply<&'a str> {
    type Output = TCallback::Output;
    fn apply(&mut self, input: &'a str) {
        for token in input.split_whitespace() {
            self.callback.apply(token);
        }
    }
}

impl<'a, TCallback> ToOperands<'a> for WhitespaceTokenizer<TCallback>
    where TCallback: ToOperands<'a> {
    fn to_operands(self) -> Vec<ChainedOperand<'a>> {
        self.callback.to_operands()
    }
}

pub struct LowercaseFilter<TCallback>
{
    callback: TCallback,
}

impl<TCallback> LowercaseFilter<TCallback> {
    pub fn create(callback: TCallback) -> Self {
        LowercaseFilter{
            callback: callback,
        }
    }
}

impl<'a, TCallback> CanApply<&'a str> for LowercaseFilter<TCallback>
    where TCallback: CanApply<String>
{
    type Output = TCallback::Output;
    fn apply(&mut self, input: &str) {
        self.callback.apply(input.to_lowercase())
    }
}

impl<'a, TCallback> ToOperands<'a> for LowercaseFilter<TCallback>
    where TCallback: ToOperands<'a> {

    fn to_operands(self) -> Vec<ChainedOperand<'a>> {
        self.callback.to_operands()
    }
}


use perlin_core::index::Index;
use std::hash::Hash;

pub struct IndexerFunnel<'a, T: 'a + Hash + Eq>
{
    doc_id: DocId,
    index: &'a mut Index<T>
}

impl<'a, T: Hash + Eq> IndexerFunnel<'a, T> {
    pub fn create(doc_id: DocId, index: &'a mut Index<T>) -> Self {
        IndexerFunnel {
            doc_id: doc_id,
            index: index
        }
    }
}

impl<'a, TTerm: 'a + Debug + Hash + Ord + Eq> CanApply<TTerm> for IndexerFunnel<'a, TTerm>{

    type Output = TTerm;
    
    fn apply(&mut self, input: TTerm) {
        self.index.index_term(input, self.doc_id);
    }
}

#[macro_export]
macro_rules! inner_pipeline {
    (;$INDEX:ident; ;$doc_id:expr; ;$field:ident;
     > $($x:tt)*) => {
        // >
        inner_pipeline!(;$INDEX; ;$doc_id; ;$field; $($x)*)
    };
    (;$INDEX:ident; ;$doc_id:expr; ;$field:ident;
     $element:ident($($param:expr),+) | [$this_field:ident] $($x:tt)*) =>
    // ;doc_id; ;field_id; Element(params) | [field]
    {
        $element::create($($param),+ ,
                         IndexerFunnel::create($doc_id, &mut $INDEX.$this_field),
                         inner_pipeline!(;$INDEX; ;$doc_id; ;$field_id; ($x)*))
    };
    (;$INDEX:ident; ;$doc_id:expr; ;$field:ident;
     $element:ident($($param:expr),+) $($x:tt)*) =>
    // ;doc_id; ;field_id; Element(params)
    {
        $element::create($($param),+ , inner_pipeline!(;$INDEX; ;$doc_id; ;$field; $($x)*))        
    };
    (;$INDEX:ident; ;$doc_id:expr; ;$field:ident;
     $element:ident | [$this_field:ident] $($x:tt)*) =>
    // ;doc_id; ;field_id; Element | [field]
    {
        $element::create(
            IndexerFunnel::create($doc_id, &mut $INDEX.$this_field),
            inner_pipeline!(;$INDEX; ;$doc_id; ;$field; $($x)*))
    };
    (;$INDEX:ident; ;$doc_id:expr; ;$field:ident;
     $element:ident $($x:tt)*) =>
    // ;doc_id; ;field_id; Element
    {
        $element::create(inner_pipeline!(;$INDEX; ;$doc_id; ;$field; $($x)*))
    };
    (;$INDEX:ident; ;$doc_id:expr; ;$field:ident;) => {
        IndexerFunnel::create($doc_id, &mut $INDEX.$field)
    };
    () => {}
}

#[macro_export]
macro_rules! pipeline {
    ($field:ident $($x:tt)*) => {
        Box::new(move |doc_id, index, content| {
            use $crate::language::CanApply;
            use $crate::language::IndexerFunnel;
            use std::marker::PhantomData;
            let mut pipe = inner_pipeline!(;index; ;doc_id; ;$field; $($x)*);
            pipe.apply(content);
            PhantomData
        })
    }
}

