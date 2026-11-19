# Models

## selfie_segmentation_landscape.onnx

MediaPipe Selfie Segmentation (landscape variant), Copyright Google LLC, Apache License 2.0.
Used for background blur and virtual background (`src/background/model.rs`, embedded with
`include_bytes!`). The licence text is in [`LICENSE-MediaPipe.txt`](LICENSE-MediaPipe.txt).

### Source

- Original: <https://storage.googleapis.com/mediapipe-assets/selfie_segmentation_landscape.tflite>
  (linked from the MediaPipe models page, `docs/solutions/models.md` in google-ai-edge/mediapipe)
- SHA-256 of the downloaded `.tflite`:
  `a77d03f4659b9f6b6c1f5106947bf40e99d7655094b6527f214ea7d451106edd`
- Licence: the model card
  (<https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Selfie%20Segmentation.pdf>,
  authors Tingbo Hou, Karthik Raveendran et al., Google, May 6 2021) states
  "Licensed under: Apache License, Version 2.0".

### Conversion

1. tf2onnx 1.17.0 (with tensorflow 2.22.0rc0, onnx 1.23.2, Python 3.14), opset 17 (13 lacks
   `HardSwish`):

   ```
   python -I -m tf2onnx.convert --tflite selfie_segmentation_landscape.tflite --output o17.onnx --opset 17
   ```

   SHA-256 of `o17.onnx`: `100376b0b817b5f937a3e4b896f84eca2385a40d26702e1b7b059bbb62a26519`.
   The tflite stores fp16 weights behind `Dequantize` ops; tf2onnx folds them, so the ONNX
   weights are plain fp32 (no `Cast` chains).

2. tf2onnx leaves the last layer, MediaPipe's custom op `Convolution2DTransposeBias`
   (custom options: padding SAME, stride 2x2; kernel 2x2, weights `[O=1,H=2,W=2,I=16]`,
   bias `[1]`), as an unknown node. A small onnx-python script replaced it with standard
   ops: the `NCHW -> NHWC` `Transpose` in front of it is bypassed, then
   `ConvTranspose(kernel_shape=[2,2], strides=[2,2], pads=[0,0,0,0])` with the weights
   transposed `[O,H,W,I] -> [I,O,H,W]` and the same bias, followed by
   `Transpose(perm=[0,2,3,1])` back to NHWC. Unused initializers and the `ai.onnx.ml` opset
   import were dropped and the result passed `onnx.checker.check_model(full_check=True)`.

3. Checks: the replacement layer matches `tf.nn.conv2d_transpose` on the real activation
   exactly (max abs diff 0); on MediaPipe's `portrait.jpg` test image the mask is a clean
   person silhouette, and tract's output matches onnxruntime 1.31.0 within 1.4e-5.

SHA-256 of the committed `selfie_segmentation_landscape.onnx`:
`7289e6a8a670e1fc0f8298a0b8b4f686c53aedfd5f05cb08f01e4ef5a0f78539`

### Input and output

- Input `input_1`: `float32[1,144,256,3]`, NHWC, RGB, values in `0.0..=1.0` (byte / 255).
- Output `segment_back`: `float32[1,144,256,1]`, person probability in `0.0..=1.0`
  (the graph ends in a `Sigmoid`, so these are probabilities, not logits).
