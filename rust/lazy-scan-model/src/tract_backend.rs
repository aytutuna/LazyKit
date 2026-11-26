use crate::{CornerModel, Heatmaps, INPUT_VALUES, ModelError, heatmaps};
use tract_onnx::prelude::*;

/// The model on tract. tract rejects the shipped file's Einsum equations, so this takes the patched model (see
/// tools/patch_einsum.py).
pub struct TractModel(std::sync::Arc<TypedRunnableModel>);

impl TractModel {
    pub fn load(onnx: &[u8]) -> Result<Self, ModelError> {
        let load = |e: TractError| ModelError::Load(e.to_string());
        let model = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(onnx))
            .map_err(load)?
            .with_input_fact(0, f32::fact([1, 3, 256, 256]).into())
            .map_err(load)?
            .into_optimized()
            .map_err(load)?
            .into_runnable()
            .map_err(load)?;
        Ok(TractModel(model))
    }
}

impl CornerModel for TractModel {
    fn run(&self, input: &[f32]) -> Result<Heatmaps, ModelError> {
        if input.len() != INPUT_VALUES {
            return Err(ModelError::Shape);
        }
        let run = |e: TractError| ModelError::Run(e.to_string());
        let tensor = Tensor::from_shape(&[1, 3, 256, 256], input).map_err(run)?;
        let out = self.0.run(tvec!(tensor.into_tvalue())).map_err(run)?;
        let heat = out[0].to_plain_array_view::<f32>().map_err(run)?;
        heatmaps(heat.iter().copied().collect(), out[0].shape())
    }
}
