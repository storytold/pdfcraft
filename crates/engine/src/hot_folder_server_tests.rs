use super::*;

fn task(base: &Path, id: &str) -> Task {
    let input = base.join(format!("{id}-in"));
    let output = base.join(format!("{id}-out"));
    fs::create_dir(&input).unwrap();
    fs::create_dir(&output).unwrap();
    Task {
        id: id.into(),
        name: id.into(),
        input,
        output,
        enabled: true,
        interval_seconds: 1,
        settle_seconds: 1,
        settings: ocr::OcrSettings { recognize: false, auto_rotate: false, deskew: false, ..Default::default() },
        ..Default::default()
    }
}

fn pdf() -> Arc<Vec<u8>> {
    Session::new().create_from_text("server", "An original survives a failed output").unwrap()
}

fn make_due(config: &Path) {
    let mut state = read_state(config).unwrap();
    for status in state.tasks.values_mut() {
        status.next_check = 0;
    }
    for observation in state.observations.values_mut() {
        observation.since = 0;
    }
    journal::save(config, &mut state).unwrap();
}

#[test]
fn offline_and_disabled_sources_cannot_stop_other_tasks_and_recover_automatically() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let remote = task(base.path(), "remote");
    let local = task(base.path(), "local");
    let mut disabled = task(base.path(), "disabled");
    disabled.enabled = false;
    fs::remove_dir(&remote.input).unwrap();
    fs::remove_dir(&disabled.input).unwrap();
    fs::remove_dir(&disabled.output).unwrap();
    let settings = Config { tasks: vec![remote.clone(), disabled, local.clone()] };
    save_json(&config, &settings).unwrap();
    validate_structure(&settings, None).unwrap();
    initialize_config(&config).unwrap();
    fs::write(local.input.join("local.pdf"), pdf().as_slice()).unwrap();
    poll(&config, None).unwrap();
    make_due(&config);
    let state = poll(&config, None).unwrap();
    assert_eq!(state.tasks["local"].completed, 1);
    assert!(local.output.join("local.pdf").exists());
    assert!(state.tasks["remote"].retry_count > 0);
    assert!(!state.tasks["remote"].blocked);
    fs::create_dir(&remote.input).unwrap();
    fs::write(remote.input.join("remote.pdf"), pdf().as_slice()).unwrap();
    make_due(&config);
    poll_task_cancelable(&config, None, "remote", &|| false).unwrap();
    make_due(&config);
    let state = poll_task_cancelable(&config, None, "remote", &|| false).unwrap();
    assert_eq!(state.tasks["remote"].completed, 1);
    assert_eq!(state.tasks["remote"].retry_count, 0);
    assert_eq!(state.tasks["local"].completed, 1);
}

#[test]
fn transient_publication_failure_retries_the_whole_group_without_skipping_sources() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut task = task(base.path(), "group");
    task.originals = Originals::Delete;
    task.output_plan.steps[0].grouping = output::Grouping::All;
    save_config(&config, &Config { tasks: vec![task.clone()] }, None).unwrap();
    fs::write(task.input.join("a.pdf"), pdf().as_slice()).unwrap();
    fs::write(task.input.join("b.pdf"), pdf().as_slice()).unwrap();
    let mut state = State::default();
    step(&task, &mut state, 10, &config, None, &|| false).unwrap();
    let moved = base.path().join("temporarily-unavailable");
    let interrupt = std::cell::Cell::new(false);
    let outage = || {
        if !interrupt.replace(true) {
            fs::rename(&task.output, &moved).unwrap();
            fs::write(&task.output, b"mount unavailable").unwrap();
        }
        false
    };
    let error = step(&task, &mut state, 13, &config, None, &outage).unwrap_err();
    assert!(error.contains("all originals retained"));
    assert!(state.observations.values().all(|observation| !observation.attempted));
    assert!(!state.tasks["group"].blocked);
    task_failure(&mut state, "group", error, now());
    journal::save(&config, &mut state).unwrap();
    fs::remove_file(&task.output).unwrap();
    fs::rename(&moved, &task.output).unwrap();
    make_due(&config);
    let state = poll(&config, None).unwrap();
    assert_eq!(state.tasks["group"].completed, 2);
    assert_eq!(fs::read_dir(&task.output).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&task.input).unwrap().count(), 0);
}

#[test]
fn permanent_bad_document_is_skipped_per_file_but_never_dropped_from_a_group() {
    for grouped in [false, true] {
        let base = tempfile::tempdir().unwrap();
        let config = base.path().join("tasks.json");
        let mut task = task(base.path(), "documents");
        if grouped {
            task.output_plan.steps[0].grouping = output::Grouping::All;
        }
        task.output_plan.steps[0].format = output::Format::Txt;
        save_config(&config, &Config { tasks: vec![task.clone()] }, None).unwrap();
        fs::write(task.input.join("a-bad.pdf"), b"malformed PDF").unwrap();
        fs::write(task.input.join("b-good.pdf"), pdf().as_slice()).unwrap();
        poll(&config, None).unwrap();
        make_due(&config);
        let failed = poll(&config, None).unwrap();
        assert_eq!(failed.tasks["documents"].blocked, grouped);
        make_due(&config);
        let retried = poll(&config, None).unwrap();
        assert_eq!(retried.tasks["documents"].completed, if grouped { 0 } else { 1 });
        assert!(task.input.join("a-bad.pdf").exists());
    }
}

#[test]
fn lifetime_receipts_are_archived_without_losing_deduplication_or_cleanup_proofs() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let task = task(base.path(), "live");
    save_config(&config, &Config { tasks: vec![task.clone()] }, None).unwrap();
    let source = task.input.join("first.pdf");
    fs::write(&source, pdf().as_slice()).unwrap();
    let mut state = State::default();
    step(&task, &mut state, 10, &config, None, &|| false).unwrap();
    step(&task, &mut state, 13, &config, None, &|| false).unwrap();
    let original = state.receipts.first_key_value().unwrap().1.clone();
    let bytes = fs::read(&source).unwrap();
    let mut first_key = None;
    // Originals=Keep is the default: finished physical inputs must not exhaust the
    // enumerator either. Every historical entry below has a real retained source.
    for index in 0..10_001 {
        let retained = task.input.join(format!("old-{index:05}.pdf"));
        fs::write(&retained, &bytes).unwrap();
        let stamp = fingerprint(&retained).unwrap();
        let key = format!("{}:{}", task.id, confined(&retained, None).unwrap().display());
        let receipt_key = format!("{key}:{}", original.source_hash);
        first_key.get_or_insert_with(|| receipt_key.clone());
        let mut receipt = original.clone();
        receipt.source_stamp = stamp.clone();
        state.receipts.insert(receipt_key.clone(), receipt);
        state.observations.insert(key, Observation { fingerprint: stamp, since: 1, attempted: true, receipt_key: Some(receipt_key) });
        if index % 256 == 255 {
            journal::save(&config, &mut state).unwrap();
        }
    }
    journal::save(&config, &mut state).unwrap();
    let mut restored = read_state(&config).unwrap();
    assert!(restored.receipts.len() <= 256);
    assert!(fs::metadata(state_path(&config)).unwrap().len() < 3 * 1024 * 1024);
    let first_key = first_key.unwrap();
    let old = journal::receipt(&config, &restored, &first_key).unwrap().unwrap();
    verify_outputs(&old, None).unwrap();
    step(&task, &mut restored, 20, &config, None, &|| false).unwrap();
    assert_eq!(fs::read_dir(&task.output).unwrap().count(), 1);
    fs::write(task.input.join("next.pdf"), pdf().as_slice()).unwrap();
    step(&task, &mut restored, 26, &config, None, &|| false).unwrap();
    step(&task, &mut restored, 29, &config, None, &|| false).unwrap();
    assert!(task.output.join("next.pdf").exists());
    // An explicit retry clears observations, never the archived content receipts.
    reset(&config, "live", None).unwrap();
    restored = read_state(&config).unwrap();
    verify_outputs(&journal::receipt(&config, &restored, &first_key).unwrap().unwrap(), None).unwrap();
    assert_eq!(fs::read_dir(&task.output).unwrap().count(), 2);
}

#[test]
fn child_failures_use_capped_backoff_and_do_not_change_output_receipts() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let task = task(base.path(), "child");
    save_config(&config, &Config { tasks: vec![task] }, None).unwrap();
    for attempt in 1..=20 {
        let before = now();
        let state = record_task_failure(&config, None, "child", "child deadline expired").unwrap();
        assert_eq!(state.tasks["child"].retry_count, attempt);
        assert!(state.tasks["child"].next_check >= before + 5);
        assert!(state.tasks["child"].next_check <= now() + 900);
        assert!(!state.tasks["child"].blocked);
        assert!(state.receipts.is_empty());
    }
}

#[test]
fn a_retried_publication_reuses_its_durable_timestamp_and_idle_polls_do_not_rewrite_state() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut task = task(base.path(), "identity");
    task.output_plan.steps[0].name_template = "{name}-{time}".into();
    save_config(&config, &Config { tasks: vec![task.clone()] }, None).unwrap();
    fs::write(task.input.join("document.pdf"), pdf().as_slice()).unwrap();
    let mut state = poll(&config, None).unwrap();
    state.tasks.get_mut("identity").unwrap().attempt_started_at = Some(1234);
    journal::save(&config, &mut state).unwrap();
    output::save(
        &[output::Input { relative: "document.pdf".into(), bytes: Arc::new(fs::read(task.input.join("document.pdf")).unwrap()) }],
        &task.output,
        &task.output_plan,
        &task.name,
        1234,
        None,
        &|| false,
    )
    .unwrap();
    record_task_failure(&config, None, "identity", "child interrupted after publication").unwrap();
    let before = fs::read(state_path(&config)).unwrap();
    let state = poll(&config, None).unwrap();
    assert_eq!(state.tasks["identity"].attempt_started_at, Some(1234));
    assert_eq!(fs::read(state_path(&config)).unwrap(), before);
    reset(&config, "identity", None).unwrap();
    assert_eq!(read_state(&config).unwrap().tasks["identity"].attempt_started_at, Some(1234));
    poll(&config, None).unwrap();
    make_due(&config);
    let resumed = poll(&config, None).unwrap();
    assert_eq!(resumed.tasks["identity"].completed, 1);
    assert_eq!(fs::read_dir(&task.output).unwrap().count(), 1);
}

#[test]
fn expanded_preprocessing_is_rejected_before_exceeding_the_remaining_batch_budget() {
    let bytes = pdf();
    let settings = preprocess::Settings { grayscale: true, ..Default::default() };
    let error = preprocess::prepare_bounded(bytes.clone(), &settings, 72.0, (bytes.len() - 1) as u64, 1234, &|| false).unwrap_err();
    assert!(error.contains("budget"));
    let error = preprocess::prepare_bounded(bytes.clone(), &settings, 300.0, bytes.len() as u64 + 128, 1234, &|| false).unwrap_err();
    assert!(error.contains("prepared"));
    let base = tempfile::tempdir().unwrap();
    let task = task(base.path(), "raster");
    let mut image = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(20, 20).write_to(&mut image, image::ImageFormat::Png).unwrap();
    let error = prepare(&task, Path::new("scan.png"), image.into_inner(), &base.path().join("tasks.json"), 1000, 1234, &|| false).err().unwrap();
    assert!(error.contains("raster memory budget"));
}

#[test]
fn interrupted_group_spill_recovers_every_receipt_as_one_transaction() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut task = task(base.path(), "aa-group");
    task.output_plan.steps[0].grouping = output::Grouping::All;
    save_config(&config, &Config { tasks: vec![task.clone()] }, None).unwrap();
    let bytes = pdf();
    for name in ["one.pdf", "two.pdf"] {
        fs::write(task.input.join(name), bytes.as_slice()).unwrap();
    }
    let inputs = ["one.pdf", "two.pdf"].map(|relative| output::Input { relative: relative.into(), bytes: bytes.clone() });
    let output = output::save(&inputs, &task.output, &task.output_plan, &task.name, 12, None, &|| false).unwrap();
    let hash: String = Sha256::digest(bytes.as_slice()).iter().map(|byte| format!("{byte:02x}")).collect();
    let mut state = State::default();
    journal::save(&config, &mut state).unwrap();
    for name in ["one.pdf", "two.pdf"] {
        let source = task.input.join(name);
        state.receipts.insert(
            format!("{}:{}:{hash}", task.id, confined(&source, None).unwrap().display()),
            Receipt {
                outputs: output.outputs.clone(),
                source_hash: hash.clone(),
                source_stamp: fingerprint(&source).unwrap(),
                output_proofs: output.proofs.clone(),
                cleanup: None,
            },
        );
    }
    let receipt = state.receipts.first_key_value().unwrap().1.clone();
    for index in 0..256 {
        state.receipts.insert(format!("zz-completed:{index}"), receipt.clone());
    }
    assert!(
        journal::save_with(&config, &mut state, &|count| if count == 1 { Err("simulated crash after one spill".into()) } else { Ok(()) }).is_err()
    );
    assert!(journal::pending_path(&state_path(&config)).exists());
    let mut restored = read_state(&config).unwrap();
    assert_eq!(restored.receipts.len(), 258);
    step(&task, &mut restored, 20, &config, None, &|| false).unwrap();
    step(&task, &mut restored, 23, &config, None, &|| false).unwrap();
    journal::save(&config, &mut restored).unwrap();
    assert!(!journal::pending_path(&state_path(&config)).exists());
    assert_eq!(fs::read_dir(&task.output).unwrap().count(), 1);
    assert!(task.input.join("one.pdf").exists());
    assert!(task.input.join("two.pdf").exists());
}

#[test]
fn changing_a_project_manifest_cannot_consume_another_tasks_originals() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut project = task(base.path(), "project");
    let local = task(base.path(), "local");
    let manifest = base.path().join("project.json");
    let original = base.path().join("original.pdf");
    fs::write(&original, pdf().as_slice()).unwrap();
    let mut contents = sources::ProjectManifest { files: vec!["original.pdf".into()], ..Default::default() };
    sources::write_project_manifest(&manifest, &contents, None).unwrap();
    project.source = sources::SourceConfig::Project(sources::ProjectSource { manifest: manifest.clone(), action: sources::SourceAction::Delete });
    save_config(&config, &Config { tasks: vec![project, local.clone()] }, None).unwrap();
    let protected = local.input.join("unprocessed.pdf");
    fs::write(&protected, pdf().as_slice()).unwrap();
    contents.files.push("local-in/unprocessed.pdf".into());
    save_json(&manifest, &contents).unwrap();
    let result = poll_task_cancelable(&config, None, "project", &|| false).unwrap();
    assert!(result.tasks["project"].error.as_ref().unwrap().contains("overlap"));
    assert!(protected.exists());
    assert!(original.exists());
    assert!(!result.tasks.contains_key("local"));
}

#[test]
fn replayed_scan_and_group_exports_keep_the_transaction_modification_date() {
    if !ocr::available() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut task = task(base.path(), "clock");
    task.settings.recognize = true;
    task.preprocessing.grayscale = true;
    task.output_plan.steps[0].grouping = output::Grouping::All;
    save_config(&config, &Config { tasks: vec![task.clone()] }, None).unwrap();
    let mut session = Session::new();
    let id = session.open("source", None, pdf(), None).unwrap();
    let png = crate::export::Exporter::new(session.get(id).unwrap()).png(0, 150.0).unwrap();
    let mut results = Vec::new();
    for _ in 0..2 {
        let prepared = prepare(&task, Path::new("scan.png"), png.clone(), &config, MAX_INPUT, 1234, &|| false).ok().unwrap().document.unwrap();
        assert_transaction_date(prepared.clone(), 1234);
        results.push(prepared);
    }
    assert_eq!(results[0], results[1]);
    let inputs: Vec<_> =
        results.into_iter().enumerate().map(|(i, bytes)| output::Input { relative: format!("scan-{i}.pdf").into(), bytes }).collect();
    let first = output::save(&inputs, &task.output, &task.output_plan, &task.name, 1234, None, &|| false).unwrap();
    let second = output::save(&inputs, &task.output, &task.output_plan, &task.name, 1234, None, &|| false).unwrap();
    assert_eq!(first.outputs, second.outputs);
    assert_eq!(fs::read_dir(&task.output).unwrap().count(), 1);
    assert_transaction_date(Arc::new(fs::read(&first.outputs[0]).unwrap()), 1234);
}

fn assert_transaction_date(bytes: Arc<Vec<u8>>, timestamp: i64) {
    let document = pdfcraft_cos::Document::open(bytes).unwrap();
    let info = document.dict(document.trailer().get(b"Info").unwrap()).unwrap();
    assert_eq!(info.get(b"ModDate").unwrap().as_string().unwrap().to_text(), pdfcraft_cos::pdf_date(timestamp));
}

#[test]
fn first_calendar_schedule_is_persisted_once_without_probing_the_input() {
    use chrono::Timelike;
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut task = task(base.path(), "calendar");
    task.schedule = Schedule::Daily { hour: (chrono::Local::now().hour() + 1) % 24, minute: 0 };
    fs::remove_dir(&task.input).unwrap();
    save_json(&config, &Config { tasks: vec![task] }).unwrap();
    let state = poll(&config, None).unwrap();
    assert!(state.tasks["calendar"].next_check > now());
    assert_eq!(state.tasks["calendar"].failed, 0);
    assert_eq!(read_state(&config).unwrap().tasks["calendar"].next_check, state.tasks["calendar"].next_check);
    let before = fs::read(state_path(&config)).unwrap();
    poll(&config, None).unwrap();
    assert_eq!(fs::read(state_path(&config)).unwrap(), before);
}

#[test]
fn root_access_keeps_its_authorized_spelling_and_refuses_traversal_and_sibling_prefixes() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("allowed");
    let sibling = base.path().join("allowed-extra");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&sibling).unwrap();
    let source = root.join("source.pdf");
    let outside = sibling.join("private.pdf");
    fs::write(&source, b"retained source").unwrap();
    fs::write(&outside, b"outside the authorized root").unwrap();
    // On Windows CI, tempdir inherits a short-name ancestor from the runner's TEMP.
    // Exercise both the caller's spelling and the filesystem's canonical spelling.
    assert_eq!(confined(&source, Some(&root)).unwrap(), source.canonicalize().unwrap());
    assert_eq!(confined(&source.canonicalize().unwrap(), Some(&root)).unwrap(), source.canonicalize().unwrap());
    assert_eq!(confined(Path::new("source.pdf"), Some(&root)).unwrap(), source.canonicalize().unwrap());
    assert_eq!(confined(&root.join("new.pdf"), Some(&root)).unwrap(), root.canonicalize().unwrap().join("new.pdf"));
    for path in [&outside, &root.join("../allowed-extra/private.pdf")] {
        assert_eq!(confined(path, Some(&root)).unwrap_err(), "hot folder path is outside the allowed directory");
    }
    assert_eq!(fs::read(source).unwrap(), b"retained source");
    assert_eq!(fs::read(outside).unwrap(), b"outside the authorized root");
}

#[cfg(windows)]
#[test]
fn windows_alias_overlap_is_deny_only_and_normalizes_drive_unc_and_extended_prefixes() {
    let cases = [
        (r"C:\Users\RUNNER~1\Temp\job\input", r"\\?\c:\Users\runneradmin\Temp\job\input\a.pdf", true),
        (r"C:\Users\RUNNER~1\Temp\job\input", r"C:\Users\runneradmin\Temp\job\output", false),
        (r"C:\MISSIN~1\in", r"C:\Missing directory\input", false),
        (r"C:\MISSIN~1\in", r"D:\Missing directory\in", false),
        (r"\\server\share\LONGFO~1\input", r"\\?\UNC\SERVER\SHARE\Long folder\input\a.pdf", true),
        (r"\\server\share\LONGFO~1\input", r"\\server\other-share\Long folder\input", false),
        (r"\\server\share\LONGFO~1\input", r"\\other-server\share\Long folder\input", false),
        (r"C:\LONGFO~1\input", r"\\server\share\Long folder\input", false),
        (r"C:\data\input", r"\\?\C:\data\input\a.pdf", true),
    ];
    for (left, right, overlap) in cases {
        assert_eq!(paths_may_overlap(Path::new(left), Path::new(right)), overlap, "{left} vs {right}");
        assert_eq!(paths_may_overlap(Path::new(right), Path::new(left)), overlap, "{right} vs {left}");
    }
    let short = Path::new(r"C:\Users\RUNNER~1\Temp\job\input");
    let long = Path::new(r"C:\Users\runneradmin\Temp\job\input");
    assert!(paths_may_overlap(short, long));
    assert!(!lexically_inside(short, long), "a potential alias does not authorize access");
    assert!(!lexically_inside(long, short), "a potential alias does not authorize access");
    assert!(paths_may_overlap(Path::new(r"C:\straße\input"), Path::new(r"C:\strasse\input")));
    assert!(!lexically_inside(Path::new(r"C:\straße\input"), Path::new(r"C:\strasse\input")));
    assert!(lexically_inside(Path::new(r"\\?\C:\data\input\a.pdf"), Path::new(r"c:\data\input")));
    assert!(lexically_inside(Path::new(r"\\?\UNC\SERVER\SHARE\input\a.pdf"), Path::new(r"\\server\share\input")));
    assert!(!lexically_inside(Path::new(r"C:\data\input-extra\a.pdf"), Path::new(r"C:\data\input")));
    for invalid in [r"C:relative", r"\root-relative", r"\\.\PhysicalDrive0", r"\\?\Volume{unknown}\input", r"C:\data\..\other"] {
        assert!(paths_may_overlap(Path::new(invalid), long), "unknown boundaries fail closed");
        assert!(!lexically_inside(Path::new(invalid), long));
    }
    for short in ["RUNNER~1", "LONGF~12", "DOCUME~1.PDF", "A~123456"] {
        assert!(possible_short_name(short), "{short}");
    }
    for ordinary in ["~1", "folder~", "folder~x", "toolong~12", "long name~1", "DOC~1.PDF.EXTRA"] {
        assert!(!possible_short_name(ordinary), "{ordinary}");
    }
}

#[cfg(windows)]
#[test]
fn windows_structure_refuses_alias_overlap_without_resolving_other_task_paths() {
    let first = Task {
        id: "first".into(),
        name: "First".into(),
        enabled: true,
        input: r"C:\MISSIN~1\input".into(),
        output: r"C:\MISSIN~1\output".into(),
        ..Default::default()
    };
    let second = Task {
        id: "second".into(),
        name: "Second".into(),
        enabled: true,
        input: r"C:\Missing directory\input\child".into(),
        output: r"C:\Missing directory\other-output".into(),
        ..Default::default()
    };
    assert!(validate_structure(&Config { tasks: vec![first.clone(), second] }, None).unwrap_err().contains("overlap"));
    let own_output = Task { output: r"C:\Missing directory\input\output".into(), ..first.clone() };
    assert!(validate_structure(&Config { tasks: vec![own_output] }, None).unwrap_err().contains("contain"));
    let remote = Task {
        id: "remote".into(),
        name: "Offline share".into(),
        enabled: true,
        input: r"\\pdfcraft-offline.invalid\share\input".into(),
        output: r"\\pdfcraft-offline.invalid\share\output".into(),
        ..Default::default()
    };
    validate_structure(&Config { tasks: vec![first, remote] }, None).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_possible_alias_never_grants_root_access_or_contacts_an_outside_share() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("A lengthy authorized folder");
    fs::create_dir(&root).unwrap();
    let original = root.join("original.pdf");
    fs::write(&original, b"retained").unwrap();
    let ambiguous = base.path().join("ALENGT~1").join("original.pdf");
    assert!(paths_may_overlap(&ambiguous, &root));
    assert!(!lexically_inside(&ambiguous, &root));
    for candidate in [ambiguous.as_path(), Path::new(r"\\pdfcraft-offline.invalid\share\original.pdf")] {
        assert_eq!(confined(candidate, Some(&root)).unwrap_err(), "hot folder path is outside the allowed directory");
    }
    assert_eq!(fs::read(original).unwrap(), b"retained");
}

#[cfg(windows)]
#[test]
fn windows_selected_local_task_progresses_with_an_unrelated_offline_unc_task() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let local = task(base.path(), "local");
    let remote = Task {
        id: "remote".into(),
        name: "Offline share".into(),
        enabled: true,
        input: r"\\pdfcraft-offline.invalid\share\input".into(),
        output: r"\\pdfcraft-offline.invalid\share\output".into(),
        ..Default::default()
    };
    save_json(&config, &Config { tasks: vec![remote, local.clone()] }).unwrap();
    let original = pdf();
    let source = local.input.join("source.pdf");
    fs::write(&source, original.as_slice()).unwrap();
    poll_task_cancelable(&config, None, "local", &|| false).unwrap();
    make_due(&config);
    let state = poll_task_cancelable(&config, None, "local", &|| false).unwrap();
    assert_eq!(state.tasks["local"].completed, 1);
    assert!(!state.tasks.contains_key("remote"));
    assert_eq!(fs::read(source).unwrap(), *original);
    assert!(local.output.join("source.pdf").is_file());
}

/// `count` receipts that each carry a grouped batch's full list of `pages` outputs, as `step`
/// writes them. A few large receipts keep the test fast; together they exceed one journal file.
fn oversized_state(count: usize, pages: usize) -> State {
    let mut state = State::default();
    let folder = "x".repeat(280);
    let outputs: Vec<PathBuf> = (0..pages).map(|page| PathBuf::from(format!("{folder}/{page:05}.tif"))).collect();
    for index in 0..count {
        let receipt =
            Receipt { outputs: outputs.clone(), source_hash: "hash".into(), source_stamp: String::new(), output_proofs: vec![], cleanup: None };
        state.receipts.insert(format!("grouped:{index:03}:hash"), receipt);
    }
    state
}

fn snapshot_files(dir: &Path) -> usize {
    fs::read_dir(dir).unwrap().filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".pending.")).count()
}

/// CodeRabbit: the write-ahead snapshot of a large grouped batch exceeded the 16 MiB journal
/// file limit, so the batch republished its outputs on every retry and never finished.
#[test]
fn a_grouped_batch_larger_than_one_journal_file_is_saved_and_spilled() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut state = oversized_state(6, 12_800);
    assert!(serde_json::to_vec(&state).unwrap().len() as u64 > MAX_JSON, "the snapshot needs several files");
    journal::save(&config, &mut state).unwrap();
    assert_eq!(snapshot_files(base.path()), 0, "the snapshot is removed once history holds it");
    let restored = read_state(&config).unwrap();
    for index in 0..6 {
        let receipt = journal::receipt(&config, &restored, &format!("grouped:{index:03}:hash")).unwrap();
        assert_eq!(receipt.map(|r| r.outputs.len()), Some(12_800), "receipt {index}");
    }
}

#[test]
fn an_interrupted_spill_of_a_large_batch_recovers_every_receipt() {
    let base = tempfile::tempdir().unwrap();
    let config = base.path().join("tasks.json");
    let mut state = oversized_state(6, 12_800);
    assert!(
        journal::save_with(&config, &mut state, &|count| if count == 1 { Err("simulated crash after one spill".into()) } else { Ok(()) }).is_err()
    );
    assert_eq!(read_state(&config).unwrap().receipts.len(), 6, "the whole group is visible together");
    // A damaged part is refused, never read as a shorter state.
    let part = fs::read_dir(base.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            name.contains(".pending.") && !name.ends_with(".json")
        })
        .unwrap();
    let mut bytes = fs::read(&part).unwrap();
    bytes[0] ^= 1;
    fs::write(&part, bytes).unwrap();
    assert!(read_state(&config).is_err());
}

/// The spill loop's running size is exactly the serialized cache's size.
#[test]
fn the_journal_measures_a_map_exactly_as_serde_writes_it() {
    let state = oversized_state(3, 10);
    let sizes = journal::entry_sizes(&state.receipts).unwrap();
    assert_eq!(journal::map_len(sizes.values().sum(), sizes.len()), serde_json::to_vec(&state.receipts).unwrap().len());
    assert_eq!(journal::map_len(0, 0), serde_json::to_vec(&BTreeMap::<String, Receipt>::new()).unwrap().len());
}
