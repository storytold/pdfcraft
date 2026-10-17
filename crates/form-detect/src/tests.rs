use super::*;

#[test]
fn decodes_classes_clips_boxes_and_suppresses_cross_class_duplicates() {
    let boxes = NdTensor::from_data(
        [1, 4, 4],
        vec![
            0.5, 0.5, 0.6, 0.2, 0.5, 0.5, 0.6, 0.2, // Same region, different class and lower score.
            0.95, 0.95, 0.2, 0.2, 0.1, 0.1, 0.1, 0.1,
        ],
    );
    let logits = NdTensor::from_data([1, 4, 3], vec![8.0, -8.0, -8.0, -8.0, 6.0, -8.0, -8.0, -8.0, 7.0, -8.0, 5.0, -8.0]);
    let found = decode(boxes.view().into(), logits.view().into(), 0.3).unwrap();
    assert_eq!(found.len(), 3);
    assert_eq!(found.iter().map(|p| p.kind).collect::<Vec<_>>(), [Kind::Text, Kind::Signature, Kind::CheckBox]);
    assert_eq!(found[1].rect[2..], [1.0, 1.0]);
    assert!((found[0].rect[0] - 0.2).abs() < 1e-6);
}

#[test]
fn unused_classes_and_degenerate_boxes_are_not_fields() {
    let boxes = NdTensor::from_data([1, 3, 4], vec![0.5, 0.5, 0.1, 0.1, 0.5, 0.5, -0.1, 0.1, 2.0, 2.0, 0.1, 0.1]);
    let logits = NdTensor::from_data([1, 3, 4], vec![0.0, 0.0, 0.0, 9.0, 9.0, 0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0]);
    assert!(decode(boxes.view().into(), logits.view().into(), 0.3).unwrap().is_empty());
}

#[test]
fn suppression_removes_partially_overlapping_duplicate_guesses() {
    let boxes = NdTensor::from_data([1, 2, 4], vec![0.3, 0.5, 0.4, 0.1, 0.55, 0.5, 0.4, 0.1]);
    let logits = NdTensor::from_data([1, 2, 3], vec![8.0, -8.0, -8.0, 7.0, -8.0, -8.0]);
    let found = decode(boxes.view().into(), logits.view().into(), 0.3).unwrap();
    assert_eq!(found.len(), 1);
    assert!((found[0].rect[0] - 0.1).abs() < 1e-6);
}

#[test]
fn malformed_outputs_are_errors_rather_than_empty_successes() {
    let boxes = NdTensor::from_data([1, 1, 4], vec![0.5, 0.5, 0.1, 0.1]);
    let mismatched = NdTensor::<f32, 3>::zeros([1, 2, 3]);
    assert!(decode(boxes.view().into(), mismatched.view().into(), 0.3).is_err());
    let nan = NdTensor::from_data([1, 1, 3], vec![f32::NAN, 0.0, 0.0]);
    assert!(decode(boxes.view().into(), nan.view().into(), 0.3).is_err());
    let scores = NdTensor::from_data([1, 1, 3], vec![9.0, 0.0, 0.0]);
    let bad_box = NdTensor::from_data([1, 1, 4], vec![f32::INFINITY, 0.5, 0.1, 0.1]);
    assert!(decode(bad_box.view().into(), scores.view().into(), 0.3).is_err());
    let oversized = NdTensor::<f32, 3>::zeros([1, MAX_QUERIES + 1, 4]);
    let oversized_scores = NdTensor::<f32, 3>::zeros([1, MAX_QUERIES + 1, 3]);
    assert!(decode(oversized.view().into(), oversized_scores.view().into(), 0.3).is_err());
}

#[test]
fn preprocessing_rejects_bad_sizes_before_allocation() {
    for (data, w, h) in [(&[][..], 0, 0), (&[0; 4][..], 2, 2), (&[][..], u32::MAX, u32::MAX), (&[][..], 4096, 4096)] {
        assert!(preprocess(data, w, h).is_err());
    }
    for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(validate_confidence(value).is_err());
    }
    assert!(validate_confidence(0.0).is_ok());
    assert!(validate_confidence(1.0).is_ok());
}

#[test]
fn transparent_premultiplied_pixels_become_white_and_channels_are_normalized() {
    let transparent = preprocess(&[0, 0, 0, 0], 1, 1).unwrap();
    let white = preprocess(&[255, 255, 255, 255], 1, 1).unwrap();
    assert_eq!(transparent, white);
    let red = preprocess(&[255, 0, 0, 255], 1, 1).unwrap();
    for channel in 0..3 {
        let value = if channel == 0 { 1.0 } else { 0.0 };
        let expected = (value - MEAN[channel]) / STD[channel];
        assert!((red[[0, channel, 0, 0]] - expected).abs() < 1e-6);
        assert_eq!(red[[0, channel, 0, 0]], red[[0, channel, IMAGE_SIZE - 1, IMAGE_SIZE - 1]]);
    }
}

#[test]
fn resize_preserves_the_reference_non_antialiased_sampling() {
    // Downsampling by three samples the middle pixel, without averaging the neighbouring
    // black stripes as an antialiased resize would. This catches a real preprocessing mismatch.
    let rgba: Vec<u8> = (0..IMAGE_SIZE * 3)
        .flat_map(|x| {
            let value = if x % 3 == 1 { 255 } else { 0 };
            [value, value, value, 255]
        })
        .collect();
    let sampled = preprocess(&rgba, IMAGE_SIZE as u32 * 3, 1).unwrap();
    let white = preprocess(&[255, 255, 255, 255], 1, 1).unwrap();
    assert_eq!(sampled, white);
}

#[test]
fn invalid_model_is_an_actionable_error() {
    let file = std::env::temp_dir().join(format!("pdfcraft-invalid-ffdetr-{}.onnx", std::process::id()));
    std::fs::write(&file, b"this is not an ONNX model").unwrap();
    let result = Detector::load(&file);
    std::fs::remove_file(&file).unwrap();
    assert!(matches!(result, Err(Error::Model(_))));
}
