//! The person segmentation model: MediaPipe Selfie Segmentation (landscape), converted to
//! ONNX and embedded in the binary (see `models/README.md` for provenance and licence).
//!
//! The model takes a 256x144 RGB image (NHWC, values in 0..=1) and returns a 256x144 map
//! of person probabilities (it ends in a sigmoid, so no extra activation is needed).

use std::sync::Arc;

use anyhow::{ensure, Context};
use tract_onnx::prelude::*;

/// Model input width in pixels.
pub const INPUT_W: usize = 256;
/// Model input height in pixels.
pub const INPUT_H: usize = 144;

static MODEL_ONNX: &[u8] = include_bytes!("../../models/selfie_segmentation_landscape.onnx");

/// A loaded, optimised segmentation model, ready to run on one frame at a time.
pub struct Segmenter {
    plan: Arc<TypedRunnableModel>,
}

impl Segmenter {
    /// Loads the embedded model (include_bytes! of ../../models/selfie_segmentation_landscape.onnx).
    pub fn load() -> anyhow::Result<Self> {
        let plan = tract_onnx::onnx()
            .model_for_read(&mut &MODEL_ONNX[..])
            .context("reading the segmentation model")?
            .with_input_fact(0, f32::fact([1, INPUT_H, INPUT_W, 3]).into())?
            .into_optimized()
            .context("optimising the segmentation model")?
            .into_runnable()?;
        Ok(Segmenter { plan })
    }

    /// `rgb`: INPUT_W*INPUT_H*3 bytes, row-major RGB. Returns INPUT_W*INPUT_H person
    /// probabilities in 0.0..=1.0 (clamped; NaN -> 0), row-major.
    pub fn run(&self, rgb: &[u8]) -> anyhow::Result<Vec<f32>> {
        ensure!(
            rgb.len() == INPUT_W * INPUT_H * 3,
            "segmenter input is {} bytes, expected {}",
            rgb.len(),
            INPUT_W * INPUT_H * 3
        );
        let pixels: Vec<f32> = rgb.iter().map(|&b| b as f32 / 255.0).collect();
        let input = Tensor::from_shape(&[1, INPUT_H, INPUT_W, 3], &pixels)?;
        let outputs = self.plan.run(tvec!(input.into()))?;
        let mask = outputs[0].try_as_plain_ram()?.as_slice::<f32>()?;
        ensure!(
            mask.len() == INPUT_W * INPUT_H,
            "segmenter output has {} values, expected {}",
            mask.len(),
            INPUT_W * INPUT_H
        );
        // f32::clamp keeps NaN, so map it to "background" explicitly.
        Ok(mask
            .iter()
            .map(|&p| if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// A smooth colour gradient, so the model sees something image-like.
    fn gradient() -> Vec<u8> {
        let mut rgb = Vec::with_capacity(INPUT_W * INPUT_H * 3);
        for y in 0..INPUT_H {
            for x in 0..INPUT_W {
                rgb.push((x * 255 / (INPUT_W - 1)) as u8);
                rgb.push((y * 255 / (INPUT_H - 1)) as u8);
                rgb.push(128);
            }
        }
        rgb
    }

    #[test]
    fn segmenter_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Segmenter>();
    }

    #[test]
    fn loads_and_returns_mask_of_right_shape() {
        let segmenter = Segmenter::load().unwrap();
        let mask = segmenter.run(&gradient()).unwrap();
        assert_eq!(mask.len(), INPUT_W * INPUT_H);
        assert!(mask.iter().all(|p| (0.0..=1.0).contains(p)));
    }

    #[test]
    fn run_rejects_wrong_input_length() {
        let segmenter = Segmenter::load().unwrap();
        assert!(segmenter.run(&[]).is_err());
        assert!(segmenter.run(&vec![0; INPUT_W * INPUT_H * 3 - 1]).is_err());
        assert!(segmenter.run(&vec![0; INPUT_W * INPUT_H * 4]).is_err());
    }

    /// Inference time; run with `cargo test --release --lib background::model -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_inference() {
        let segmenter = Segmenter::load().unwrap();
        let rgb = gradient();
        for _ in 0..5 {
            segmenter.run(&rgb).unwrap();
        }
        let mut times = Vec::new();
        for _ in 0..30 {
            let start = Instant::now();
            segmenter.run(&rgb).unwrap();
            times.push(start.elapsed());
        }
        let mean = times.iter().sum::<Duration>() / times.len() as u32;
        let max = times.iter().max().unwrap();
        println!(
            "segmenter inference over {} runs: mean {:.2} ms, max {:.2} ms",
            times.len(),
            mean.as_secs_f64() * 1000.0,
            max.as_secs_f64() * 1000.0
        );
    }
}
