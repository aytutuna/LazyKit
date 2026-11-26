use crate::{CornerModel, Heatmaps, INPUT_VALUES, ModelError, heatmaps};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::TensorRef;
#[cfg(feature = "ort")]
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The model on ONNX Runtime 1.24.2, loaded from the library the app ships or linked in. It reads the shipped model as it is.
pub struct OrtModel {
    session: Mutex<Session>,
    _environment: Arc<Environment>,
}

// ONNX Runtime allows one environment per process, so models loaded later share the first one (and its library).
static ENVIRONMENT: Mutex<Option<Arc<Environment>>> = Mutex::new(None);

impl OrtModel {
    /// `library` is onnxruntime.dll, libonnxruntime.so or libonnxruntime.dylib; `threads` 0 lets the runtime choose.
    #[cfg(feature = "ort")]
    pub fn load(library: &Path, onnx: &[u8], threads: usize) -> Result<Self, ModelError> {
        let environment = shared_environment(|| {
            ort::environment::init_from(library)
                .map_err(load_error)?
                // the official Windows build sends usage events to Microsoft unless told not to
                .with_telemetry(false)
                .build()
                .map_err(load_error)
        })?;
        Self::open(environment, onnx, threads)
    }

    /// The model on the ONNX Runtime the program was linked with (iOS: the xcframework of the Microsoft.ML.OnnxRuntime
    /// package, found through ORT_IOS_XCFWK_LOCATION at build time).
    #[cfg(feature = "ort-linked")]
    pub fn load_linked(onnx: &[u8], threads: usize) -> Result<Self, ModelError> {
        let environment = shared_environment(|| {
            ort::environment::init()
                .with_telemetry(false)
                .build()
                .map_err(load_error)
        })?;
        Self::open(environment, onnx, threads)
    }

    fn open(
        environment: Arc<Environment>,
        onnx: &[u8],
        threads: usize,
    ) -> Result<Self, ModelError> {
        let session = Session::builder(&environment)
            .map_err(load_error)?
            .with_intra_threads(threads)
            .map_err(load_error)?
            .commit_from_memory(onnx)
            .map_err(load_error)?;
        Ok(OrtModel {
            session: Mutex::new(session),
            _environment: environment,
        })
    }
}

fn load_error(e: impl std::fmt::Display) -> ModelError {
    ModelError::Load(e.to_string())
}

fn shared_environment(
    build: impl FnOnce() -> Result<Environment, ModelError>,
) -> Result<Arc<Environment>, ModelError> {
    let mut shared = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(environment) = shared.as_ref() {
        return Ok(environment.clone());
    }
    Ok(shared.insert(Arc::new(build()?)).clone())
}

impl CornerModel for OrtModel {
    fn run(&self, input: &[f32]) -> Result<Heatmaps, ModelError> {
        if input.len() != INPUT_VALUES {
            return Err(ModelError::Shape);
        }
        let run = |e: ort::Error| ModelError::Run(e.to_string());
        let tensor = TensorRef::from_array_view(([1usize, 3, 256, 256], input)).map_err(run)?;
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let out = session.run(ort::inputs!["img" => tensor]).map_err(run)?;
        let (shape, data) = out
            .get("heatmap")
            .ok_or(ModelError::Shape)?
            .try_extract_tensor::<f32>()
            .map_err(run)?;
        let shape: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        heatmaps(data.to_vec(), &shape)
    }
}
