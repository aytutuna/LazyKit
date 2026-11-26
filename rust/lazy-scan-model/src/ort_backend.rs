use crate::{CornerModel, Heatmaps, INPUT_VALUES, ModelError, heatmaps};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The model on ONNX Runtime, loaded from the library the app ships (1.24.2). It reads the shipped model as it is.
pub struct OrtModel {
    session: Mutex<Session>,
    _environment: Arc<Environment>,
}

// ONNX Runtime allows one environment per process, so models loaded later share the first one (and its library).
static ENVIRONMENT: Mutex<Option<Arc<Environment>>> = Mutex::new(None);

impl OrtModel {
    /// `library` is onnxruntime.dll, libonnxruntime.so or libonnxruntime.dylib; `threads` 0 lets the runtime choose.
    pub fn load(library: &Path, onnx: &[u8], threads: usize) -> Result<Self, ModelError> {
        let load = |e: &dyn std::fmt::Display| ModelError::Load(e.to_string());
        let mut shared = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
        let environment = match shared.as_ref() {
            Some(environment) => environment.clone(),
            None => {
                let built = ort::environment::init_from(library)
                    .map_err(|e| load(&e))?
                    // the official Windows build sends usage events to Microsoft unless told not to
                    .with_telemetry(false)
                    .build()
                    .map_err(|e| load(&e))?;
                shared.insert(Arc::new(built)).clone()
            }
        };
        let session = Session::builder(&environment)
            .map_err(|e| load(&e))?
            .with_intra_threads(threads)
            .map_err(|e| load(&e))?
            .commit_from_memory(onnx)
            .map_err(|e| load(&e))?;
        Ok(OrtModel {
            session: Mutex::new(session),
            _environment: environment,
        })
    }
}

impl CornerModel for OrtModel {
    fn run(&self, input: &[f32]) -> Result<Heatmaps, ModelError> {
        if input.len() != INPUT_VALUES {
            return Err(ModelError::Shape);
        }
        let run = |e: ort::Error| ModelError::Run(e.to_string());
        let tensor = Tensor::from_array(([1usize, 3, 256, 256], input.to_vec().into_boxed_slice()))
            .map_err(run)?;
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
