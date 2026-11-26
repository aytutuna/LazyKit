// DocAligner's model on the SmartDoc frames of testdata/scan/pages: every corner within 1.5 % of the photo's diagonal of the
// ground truth (Lazy Scan's tolerance), the page taken, and the two runtimes agreeing. Needs rust/lazy-scan-model/scripts/fetch.sh
// to have run; skipped without the models unless LAZY_SCAN_REQUIRE_MODEL=1, which CI sets.
#![cfg(any(feature = "tract", feature = "ort", feature = "ort-linked"))]

mod common;

use lazy_image::Rgba;
use lazy_scan::{CornerModel, Point, Quad, decode_heatmap, locate, model_input};
use std::path::PathBuf;

const TOLERANCE: f64 = 0.015;

struct Photo {
    file: String,
    image: Rgba,
    truth: Quad,
}

fn models_dir() -> PathBuf {
    std::env::var_os("LAZY_SCAN_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../lazy-scan-model/models")
        })
}

fn available(name: &str) -> Option<Vec<u8>> {
    match std::fs::read(models_dir().join(name)) {
        Ok(bytes) => Some(bytes),
        Err(_) => {
            assert!(
                std::env::var("LAZY_SCAN_REQUIRE_MODEL").as_deref() != Ok("1"),
                "{name} is not fetched"
            );
            eprintln!("{name} is not fetched (rust/lazy-scan-model/scripts/fetch.sh), skipping");
            None
        }
    }
}

fn photos() -> Vec<Photo> {
    let list = common::json("pages/corners.json");
    list.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let file = e["file"].as_str().unwrap().to_string();
            let mut decoder = jpeg_decoder::Decoder::new(
                std::fs::File::open(common::testdata(&format!("pages/{file}")))
                    .map(std::io::BufReader::new)
                    .unwrap(),
            );
            let rgb = decoder.decode().unwrap();
            let info = decoder.info().unwrap();
            assert_eq!(
                (info.width as u64, info.height as u64),
                (e["width"].as_u64().unwrap(), e["height"].as_u64().unwrap())
            );
            let mut data = Vec::with_capacity(rgb.len() / 3 * 4);
            for p in rgb.as_chunks::<3>().0.iter() {
                data.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
            let point = |k: &str| Point::new(e[k][0].as_f64().unwrap(), e[k][1].as_f64().unwrap());
            let truth = Quad::from_corners([
                point("topLeft"),
                point("topRight"),
                point("bottomRight"),
                point("bottomLeft"),
            ]);
            Photo {
                file,
                image: Rgba::new(info.width as usize, info.height as usize, data),
                truth,
            }
        })
        .collect()
}

// the corners the model gives, named by position, in the photo's pixels
fn corners(model: &dyn CornerModel, photo: &Photo) -> Quad {
    let heat = model.run(&model_input(&photo.image)).unwrap();
    let found = decode_heatmap(&heat.values, heat.width, heat.height)
        .unwrap_or_else(|| panic!("no page in {}", photo.file));
    Quad::from_corners(
        found
            .quad
            .scaled(photo.image.width as f64, photo.image.height as f64)
            .0,
    )
}

fn check(name: &str, model: &dyn CornerModel, photos: &[Photo]) -> Vec<Quad> {
    photos
        .iter()
        .map(|photo| {
            let quad = corners(model, photo);
            let diagonal = (photo.image.width as f64).hypot(photo.image.height as f64);
            let worst = quad.max_corner_distance(&photo.truth);
            eprintln!(
                "{name} {}: worst corner {worst:.2} px ({:.2} % of the diagonal)",
                photo.file,
                worst / diagonal * 100.0
            );
            assert!(worst / diagonal < TOLERANCE, "{name} {}", photo.file);
            let located = locate(&photo.image, model).unwrap();
            assert!(
                located.is_some(),
                "{name}: the page in {} was not taken",
                photo.file
            );
            quad
        })
        .collect()
}

fn same(a: &[Quad], b: &[Quad], within: f64) {
    for (x, y) in a.iter().zip(b) {
        assert!(
            x.max_corner_distance(y) <= within,
            "{} px apart",
            x.max_corner_distance(y)
        );
    }
}

#[cfg(feature = "tract")]
fn tract() -> Option<(lazy_scan_model::TractModel, Vec<Quad>)> {
    let bytes = available("fastvit_t8_h_e_bifpn_256_fp32-tract.onnx")?;
    let model = lazy_scan_model::TractModel::load(&bytes).unwrap();
    let found = check("tract", &model, &photos());
    Some((model, found))
}

#[cfg(feature = "ort")]
fn ort(file: &str) -> Option<(lazy_scan_model::OrtModel, Vec<Quad>)> {
    let library = std::env::var_os("ORT_DYLIB_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            [
                "onnxruntime.dll",
                "libonnxruntime.so",
                "libonnxruntime.dylib",
            ]
            .iter()
            .map(|n| models_dir().join(n))
            .find(|p| p.exists())
        });
    let Some(library) = library else {
        assert!(
            std::env::var("LAZY_SCAN_REQUIRE_MODEL").as_deref() != Ok("1"),
            "ONNX Runtime is not fetched"
        );
        eprintln!("ONNX Runtime is not fetched, skipping");
        return None;
    };
    let bytes = available(file)?;
    let model = lazy_scan_model::OrtModel::load(&library, &bytes, 0).unwrap();
    let found = check("ort", &model, &photos());
    Some((model, found))
}

// the build for iOS links ONNX Runtime in; the test is built there and run where a simulator is at hand
#[cfg(feature = "ort-linked")]
#[test]
fn linked_ort_finds_every_page() {
    let Some(bytes) = available("fastvit_t8_h_e_bifpn_256_fp32.onnx") else {
        return;
    };
    let model = lazy_scan_model::OrtModel::load_linked(&bytes, 0).unwrap();
    check("linked ort", &model, &photos());
}

#[cfg(feature = "tract")]
#[test]
fn tract_finds_every_page() {
    tract();
}

#[cfg(feature = "ort")]
#[test]
fn ort_finds_every_page_with_the_shipped_model() {
    ort("fastvit_t8_h_e_bifpn_256_fp32.onnx");
}

#[cfg(feature = "ort")]
#[test]
fn ort_gives_the_same_corners_with_the_patched_model() {
    let Some((_, shipped)) = ort("fastvit_t8_h_e_bifpn_256_fp32.onnx") else {
        return;
    };
    let Some((_, patched)) = ort("fastvit_t8_h_e_bifpn_256_fp32-tract.onnx") else {
        return;
    };
    same(&shipped, &patched, 0.01);
}

#[cfg(all(feature = "tract", feature = "ort"))]
#[test]
fn tract_and_ort_agree() {
    let Some((_, with_tract)) = tract() else {
        return;
    };
    let Some((_, with_ort)) = ort("fastvit_t8_h_e_bifpn_256_fp32.onnx") else {
        return;
    };
    same(&with_tract, &with_ort, 0.05);
}
