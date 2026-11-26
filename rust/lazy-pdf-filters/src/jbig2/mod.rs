// See budget.rs for the licence.

//! JBIG2Decode (ISO 32000-1 7.4.7; ITU-T T.88): the segments of an embedded stream, the globals first,
//! drawn into one page that comes out as packed rows with 0 for black, as an image stream expects.

#![allow(dead_code)]

mod arith;
mod bitmap;
mod budget;
mod generic;
mod halftone;
mod huffman;
mod symbols;
mod text;
