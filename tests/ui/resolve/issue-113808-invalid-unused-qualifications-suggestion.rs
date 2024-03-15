//@ run-rustfix

#![allow(dead_code)]
#![deny(unused_qualifications)]
#![feature(unsized_fn_params)]

#[allow(unused_imports)]
use std::ops;
use std::ops::Index;

pub struct A;

impl ops::Index<str> for A {
    //~^ ERROR unnecessary qualification
    type Output = ();
    fn index(&self, _: str) -> &Self::Output {
        &()
    }
}

// This is used to make `use std::ops::Index;` not unused_import.
// details in fix(#122373) for issue #121331
pub struct C;
impl Index<str> for C {
    type Output = ();
    fn index(&self, _: str) -> &Self::Output {
        &()
    }
}

mod inner {
    pub trait Trait<T> {}
}

// the import needs to be here for the lint to show up
#[allow(unused_imports)]
use inner::Trait;

impl inner::Trait<u8> for () {}
//~^ ERROR unnecessary qualification

impl Trait<A> for A {}
fn main() {}
