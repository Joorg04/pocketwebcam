//! Background blur and virtual background: an on-device person segmentation model
//! (MediaPipe Selfie Segmentation, run with tract) finds the person in each frame so the
//! rest can be blurred or replaced. Built only with the `background` Cargo feature.

pub mod builtin;
pub mod composite;
pub mod model;
mod processor;

pub use processor::{check_image, Processor, MAX_IMAGE_SIDE};
