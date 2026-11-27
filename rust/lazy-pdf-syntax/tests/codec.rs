mod common;

use common::open;
use common::pdf::{TestPdf, deflate};
use lazy_pdf_syntax::{Object, Parm, Problem};

fn image_file(entries: &str, data: &[u8]) -> Vec<u8> {
    let mut pdf = TestPdf::with_pages(1);
    pdf.stream_with(20, entries, data, None, true);
    pdf.finish()
}

fn stream_20(document: &mut lazy_pdf_syntax::Document) -> lazy_pdf_syntax::Stream {
    match &*document.get(20).unwrap() {
        Object::Stream(stream) => stream.clone(),
        _ => panic!("not a stream"),
    }
}

#[test]
fn a_chain_is_decoded_as_far_as_its_image_codec() {
    let mut document = open(image_file(
        "/Filter [/FlateDecode /DCTDecode] /DecodeParms [null <</ColorTransform 0>>]",
        &deflate(b"jpeg bytes"),
    ));
    let stream = stream_20(&mut document);
    let decoded = document.stream_data_to_codec(&stream, 1 << 20).unwrap();
    assert_eq!(decoded.data, b"jpeg bytes");
    let codec = decoded.codec.unwrap();
    assert_eq!(codec.filter, b"DCTDecode");
    assert_eq!(codec.parms, [(b"ColorTransform".to_vec(), Parm::Int(0))]);
    let error = document.stream_data(&stream).unwrap_err();
    assert_eq!(error.problem, Problem::Unsupported);
}

#[test]
fn a_chain_without_a_codec_is_decoded_whole() {
    let mut document = open(image_file("/Filter /FlateDecode", &deflate(b"plain")));
    let stream = stream_20(&mut document);
    let decoded = document.stream_data_to_codec(&stream, 1 << 20).unwrap();
    assert_eq!(decoded.data, b"plain");
    assert!(decoded.codec.is_none());
}

#[test]
fn the_filter_chain_lists_each_stage_with_its_parameters() {
    let mut document = open(image_file(
        "/Filter [/FlateDecode /JPXDecode] /DecodeParms [<</Predictor 12>> null]",
        &deflate(b"x"),
    ));
    let stream = stream_20(&mut document);
    let chain = document.filter_chain(&stream).unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].filter, b"FlateDecode");
    assert_eq!(chain[0].parms, [(b"Predictor".to_vec(), Parm::Int(12))]);
    assert_eq!(chain[1].filter, b"JPXDecode");
    assert!(chain[1].parms.is_empty());
}

#[test]
fn a_codec_stream_longer_than_the_limit_is_refused() {
    let mut document = open(image_file("/Filter /JPXDecode", &vec![7; 300_000]));
    let stream = stream_20(&mut document);
    assert_eq!(
        document
            .stream_data_to_codec(&stream, 100_000)
            .unwrap_err()
            .problem,
        Problem::Damaged
    );
    assert!(document.stream_data_to_codec(&stream, 400_000).is_ok());
}

#[test]
fn a_chain_of_many_stages_is_refused() {
    let names = "/FlateDecode ".repeat(17);
    let mut document = open(image_file(&format!("/Filter [{names}]"), &deflate(b"x")));
    let stream = stream_20(&mut document);
    assert_eq!(
        document.stream_data(&stream).unwrap_err().problem,
        Problem::Damaged
    );
    assert!(document.filter_chain(&stream).is_err());
}

struct Same;

impl lazy_pdf_syntax::Filters for Same {
    fn decode(
        &self,
        _filter: &[u8],
        _parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        _max_size: usize,
        _strict: bool,
    ) -> Result<Vec<u8>, lazy_pdf_syntax::DecodeError> {
        Ok(data.to_vec())
    }

    fn flate(&self, data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }
}

#[test]
fn the_stages_of_a_chain_share_one_output_budget() {
    let names = "/RL ".repeat(16);
    let file = image_file(&format!("/Filter [{names}]"), &vec![0xFF; 100_000]);
    let mut document = lazy_pdf_syntax::Document::open(file, Box::new(Same)).unwrap();
    let stream = stream_20(&mut document);
    assert!(document.stream_data_limited(&stream, 200_000, false).is_err());
    assert!(document.stream_data_limited(&stream, 1_000_000, false).is_ok());
}
