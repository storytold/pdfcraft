use super::super::recognition_args;
use super::*;
use crate::{Line, Recognizer};
use std::path::PathBuf;

fn image() -> OcrImage {
    let mut rgba = vec![255; 6 * 5 * 4];
    rgba.get_mut((2 * 6 + 3) * 4..(2 * 6 + 3) * 4 + 4).unwrap().copy_from_slice(&[25, 40, 60, 255]);
    OcrImage::new(6, 5, rgba).unwrap()
}

fn row(text: &str, confidence: &str) -> String {
    format!("5\t1\t1\t1\t1\t1\t0\t0\t4\t4\t{confidence}\t{text}\n")
}

#[test]
fn transparent_hidden_ink_is_not_a_word_and_opaque_rgb_is_preserved() {
    let transparent = OcrImage::new(1, 1, vec![0, 0, 0, 0]).unwrap();
    assert!(tight_ink(&transparent).is_err());
    let ink = tight_ink(&image()).unwrap();
    assert_eq!((ink.width, ink.height), (1, 1));
    assert_eq!(ink.rgb, [25, 40, 60]);
    let half = OcrImage::new(1, 1, vec![0, 0, 0, 128]).unwrap();
    assert_eq!(tight_ink(&half).unwrap().rgb, [127, 127, 127]);
}

#[test]
fn fixed_luminance_threshold_only_detects_the_box_never_changes_ink() {
    assert!(tight_ink(&OcrImage::new(1, 1, vec![245, 245, 245, 255]).unwrap()).is_err());
    assert_eq!(tight_ink(&OcrImage::new(1, 1, vec![244, 244, 244, 255]).unwrap()).unwrap().rgb, [244, 244, 244]);
    // Saturated blue is dark in luminance even though its maximum channel is 255.
    assert_eq!(tight_ink(&OcrImage::new(1, 1, vec![0, 0, 255, 255]).unwrap()).unwrap().rgb, [0, 0, 255]);
}

#[test]
fn fixed_borders_keep_inner_pixels_and_fill_the_surround_with_white() {
    let ink = tight_ink(&image()).unwrap();
    for border in [8, 16] {
        let raster = padded(&ink, border).unwrap();
        assert_eq!((raster.width, raster.height), (1 + 2 * border, 1 + 2 * border));
        for (index, rgb) in raster.rgb.chunks_exact(3).enumerate() {
            let x = index % raster.width as usize;
            let y = index / raster.width as usize;
            assert_eq!(rgb, if x == border as usize && y == border as usize { &[25, 40, 60] } else { &[255, 255, 255] });
        }
    }
    assert!(padded(&ink, u32::MAX).is_err());
}

#[test]
fn explicit_word_args_leave_page_recognition_argv_unchanged() {
    assert_eq!(word_args("word.png", "script/Latin"), ["word.png", "stdout", "-l", "script/Latin", "--psm", "8", "-c", "tessedit_create_tsv=1"]);
    assert_eq!(recognition_args("page.png", "tur"), ["page.png", "stdout", "-l", "tur", "-c", "tessedit_create_tsv=1"]);
}

#[test]
fn mocked_processes_return_separate_disagreeing_readings_without_touching_input() {
    let image = image();
    let before = image.clone();
    let mut calls = Vec::new();
    let candidates = proposals(&image, Instant::now(), TIMEOUT, |raster, padding, left| {
        assert!(left <= TIMEOUT);
        assert_eq!(raster.width, 1 + padding * 2);
        calls.push(padding);
        Ok(if padding == 8 { row("MAHKÜÛM.", "12") } else { row("MAHKÛM.", "91") })
    })
    .unwrap();
    assert_eq!(calls, [8, 16]);
    assert_eq!(image, before);
    assert_eq!(
        candidates,
        [
            WordCandidate { text: "MAHKÜÛM.".into(), confidence: Some(12.0), padding: 8, source: "tesseract".into() },
            WordCandidate { text: "MAHKÛM.".into(), confidence: Some(91.0), padding: 16, source: "tesseract".into() },
        ]
    );
}

#[test]
fn only_complete_single_words_are_proposals_and_unicode_pairs_are_never_stripped() {
    assert_eq!(candidate(&row("ÜÛ", "92"), 8).unwrap().unwrap().text, "ÜÛ");
    assert!(candidate(&(row("one", "92") + &row("two", "93")), 8).unwrap().is_none());
    assert!(candidate(TSV_HEADER, 8).unwrap().is_none());
    assert!(candidate(&row("two words", "90"), 8).unwrap().is_none());
    assert!(candidate("plain text", 8).is_err());
    assert!(candidate(&row(&"x".repeat(MAX_WORD_TEXT + 1), "90"), 8).is_err());
    assert_eq!(candidate(&row("word", "-1"), 8).unwrap().unwrap().confidence, None);
    assert!(candidate(&row("word", "NaN"), 8).is_err());
}

#[test]
fn candidate_rejects_control_text_and_extra_tsv_columns_without_truncation() {
    assert!(candidate(&row("wo\0rd", "90"), 8).unwrap().is_none());
    assert!(candidate(&row("word\textra", "90"), 8).is_err());
}

#[test]
fn oversized_and_blank_inputs_fail_before_a_mocked_process_can_run() {
    let oversized = OcrImage::new(2001, 1, vec![255; 2001 * 4]).unwrap();
    assert!(proposals(&oversized, Instant::now(), TIMEOUT, |_, _, _| panic!("oversized crop must not spawn")).is_err());
    let blank = OcrImage::new(1, 1, vec![255; 4]).unwrap();
    assert!(proposals(&blank, Instant::now(), TIMEOUT, |_, _, _| panic!("blank crop must not spawn")).is_err());
    let engine = TesseractCli::new(PathBuf::from("must-not-start"));
    assert!(engine.word_candidates(&oversized, &RecognizeOptions::default()).is_err());
}

#[test]
fn shared_budget_includes_discovery_time_and_second_failure_is_not_partial_success() {
    let started = Instant::now().checked_sub(Duration::from_secs(121)).unwrap();
    assert!(matches!(proposals(&image(), started, TIMEOUT, |_, _, _| panic!("expired discovery must prevent reads")), Err(OcrError::Timeout(_))));
    let started = Instant::now().checked_sub(Duration::from_secs(119)).unwrap();
    let mut calls = 0;
    let result = proposals(&image(), started, TIMEOUT, |_, _, left| {
        assert!(left <= Duration::from_secs(1), "both processes inherit only the shared remaining budget");
        calls += 1;
        if calls == 1 { Ok(row("one", "90")) } else { Err(OcrError::Process("synthetic second process failure".into())) }
    });
    assert!(matches!(result, Err(OcrError::Process(_))));
    assert_eq!(calls, 2);
}

#[test]
fn unsupported_recognizers_fail_without_falling_back_to_page_recognition() {
    struct Unsupported;
    impl Recognizer for Unsupported {
        fn id(&self) -> &'static str {
            "unsupported"
        }
        fn available(&self) -> bool {
            false
        }
        fn languages(&self) -> Vec<(String, String)> {
            Vec::new()
        }
        fn recognize(&self, _: &OcrImage, _: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
            panic!("candidate request must not fall back")
        }
    }
    assert!(Unsupported.word_candidates(&image(), &RecognizeOptions::default()).unwrap_err().to_string().contains("does not support"));
}

#[cfg(unix)]
#[test]
fn real_process_mock_enforces_psm_and_exposes_individual_crop_confidence() {
    use std::os::unix::fs::PermissionsExt;
    let directory = TempDir::create().unwrap();
    let script = directory.path.join("candidate-reader");
    std::fs::write(&script, "#!/bin/sh\nif [ \"$5\" != --psm ] || [ \"$6\" != 8 ] || [ \"$7\" != -c ] || [ \"$8\" != tessedit_create_tsv=1 ]; then exit 7; fi\nprintf '5\\t1\\t1\\t1\\t1\\t1\\t0\\t0\\t4\\t4\\t37\\tÜÛ\\n'\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let engine = TesseractCli::new(script);
    let result = engine.word_candidates(&image(), &RecognizeOptions::default()).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result.iter().map(|x| x.padding).collect::<Vec<_>>(), [8, 16]);
    assert!(result.iter().all(|x| x.text == "ÜÛ" && x.confidence == Some(37.0)));
}
