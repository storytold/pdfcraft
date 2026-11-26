//! Media settings PowerPoint keeps in the timing tree, spliced into an exported deck and read back.

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_geom::Xfrm;
use deckcraft_model::{MediaClip, ShapeId, ShapeKind, defaults};

fn node(tag: &str, attrs: &str, spid: u32, media: &str, ctn: &str) -> String {
    format!(
        r#"<p:par><p:cTn id="2" fill="hold"><p:childTnLst><p:{tag}{attrs}><p:cMediaNode{media}><p:cTn id="3"{ctn}/><p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl></p:cMediaNode></p:{tag}></p:childTnLst></p:cTn></p:par>"#
    )
}

fn clips(nodes: impl Fn(u32, u32) -> String) -> Vec<MediaClip> {
    let mut p = defaults::new_presentation(None);
    let video = p.add_media("clip.mp4", "video/mp4", b"video".to_vec()).expect("media");
    let audio = p.add_media("clip.m4a", "audio/mp4", b"audio".to_vec()).expect("media");
    let mut ids = vec![];
    for (media, is_video) in [(video, true), (audio, false)] {
        let id = ShapeId(p.alloc_id().expect("id"));
        ids.push(id.0);
        let s = deckcraft_model::Shape {
            id,
            name: "Clip".into(),
            xfrm: Some(Xfrm::new(40.0, 40.0, 200.0, 100.0)),
            kind: ShapeKind::Media(MediaClip { media, video: is_video, volume: 1.0, ..Default::default() }),
            ..Default::default()
        };
        Arc::make_mut(&mut p.slides[0]).shapes.push(s);
    }
    let timing = format!(
        r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst>{}</p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"#,
        nodes(ids[0], ids[1])
    );
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(&bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        if f.name() == "ppt/slides/slide1.xml" {
            let s = String::from_utf8(d).expect("utf8");
            assert!(!s.contains("<p:timing"));
            let end = s.rfind("</p:sld>").expect("end");
            d = format!("{}{timing}{}", &s[..end], &s[end..]).into_bytes();
        }
        out.start_file(f.name(), zip::write::SimpleFileOptions::default()).expect("start");
        out.write_all(&d).expect("write");
    }
    let q = deckcraft_pptx::import(&out.finish().expect("finish").into_inner()).expect("import");
    q.slides[0]
        .shapes
        .iter()
        .filter_map(|s| match &s.kind {
            ShapeKind::Media(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn video_settings_are_read_from_the_timing_tree() {
    let c = clips(|v, _| node("video", r#" fullScrn="1""#, v, r#" vol="80000" showWhenStopped="0""#, r#" repeatCount="indefinite""#));
    assert_eq!((c[0].volume, c[0].hide_while_not_playing, c[0].loop_play, c[0].full_screen), (0.8, true, true, true));
    // The sound next to it has no node: defaults stay.
    assert_eq!((c[1].volume, c[1].hide_while_not_playing, c[1].loop_play, c[1].full_screen), (1.0, false, false, false));
}

#[test]
fn muted_audio_has_no_volume_and_other_clips_keep_theirs() {
    let c = clips(|_, a| node("audio", "", a, r#" vol="50000" mute="1""#, ""));
    assert_eq!(c[0].volume, 1.0);
    assert_eq!((c[1].volume, c[1].loop_play, c[1].full_screen), (0.0, false, false));
}

#[test]
fn media_autoplay_respects_ancestor_conditions_and_delays() {
    let immediate = clips(|v, a| {
        format!(
            r#"<p:par><p:cTn id="2" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:video><p:cMediaNode><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cMediaNode></p:video></p:childTnLst></p:cTn></p:par><p:par><p:cTn id="4" fill="hold"><p:stCondLst><p:cond evt="onBegin" delay="0"/></p:stCondLst><p:childTnLst><p:audio><p:cMediaNode><p:cTn id="5" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{a}"/></p:tgtEl></p:cMediaNode></p:audio></p:childTnLst></p:cTn></p:par>"#
        )
    });
    assert!(immediate[0].autoplay, "delay=0 ancestor allows autoplay");
    assert!(immediate[1].autoplay, "untargeted onBegin delay=0 ancestor allows autoplay");

    let delayed_and_onclick = clips(|v, a| {
        format!(
            r#"<p:par><p:cTn id="2" fill="hold"><p:stCondLst><p:cond delay="2000"/></p:stCondLst><p:childTnLst><p:video><p:cMediaNode><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cMediaNode></p:video></p:childTnLst></p:cTn></p:par><p:par><p:cTn id="4" fill="hold"><p:stCondLst><p:cond evt="onClick" delay="0"/></p:stCondLst><p:childTnLst><p:audio><p:cMediaNode><p:cTn id="5" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{a}"/></p:tgtEl></p:cMediaNode></p:audio></p:childTnLst></p:cTn></p:par>"#
        )
    });
    assert!(!delayed_and_onclick[0].autoplay, "delayed ancestor blocks autoplay");
    assert!(!delayed_and_onclick[1].autoplay, "onClick ancestor blocks autoplay");

    let targeted_tn_and_own_cond = clips(|v, a| {
        format!(
            r#"<p:par><p:cTn id="2" fill="hold"><p:stCondLst><p:cond evt="onBegin" delay="0"><p:tn val="2"/></p:cond></p:stCondLst><p:childTnLst><p:video><p:cMediaNode><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cMediaNode></p:video></p:childTnLst></p:cTn></p:par><p:par><p:cTn id="4" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:audio><p:cMediaNode><p:cTn id="5" fill="hold"><p:stCondLst><p:cond evt="onBegin" delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{a}"/></p:tgtEl></p:cMediaNode></p:audio></p:childTnLst></p:cTn></p:par>"#
        )
    });
    assert!(!targeted_tn_and_own_cond[0].autoplay, "targeted onBegin ancestor blocks autoplay");
    assert!(!targeted_tn_and_own_cond[1].autoplay, "media cond with explicit evt is not autoplay");

    let delayed_media_cond = clips(|v, _| {
        format!(
            r#"<p:par><p:cTn id="2" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:video><p:cMediaNode><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="500"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cMediaNode></p:video></p:childTnLst></p:cTn></p:par>"#
        )
    });
    assert!(!delayed_media_cond[0].autoplay, "non-zero media cond delay is not autoplay");
}

#[test]
fn media_only_slide_exports_and_round_trips_video_and_audio_timing_nodes() {
    let mut p = defaults::new_presentation(None);
    let video = p.add_media("clip.mp4", "video/mp4", b"video".to_vec()).expect("media");
    let audio = p.add_media("clip.m4a", "audio/mp4", b"audio".to_vec()).expect("media");
    let vid_id = ShapeId(p.alloc_id().expect("id"));
    let aud_id = ShapeId(p.alloc_id().expect("id"));
    let s0 = Arc::make_mut(&mut p.slides[0]);
    s0.shapes.push(deckcraft_model::Shape {
        id: vid_id,
        name: "AutoplayVideo".into(),
        xfrm: Some(Xfrm::new(40.0, 40.0, 200.0, 100.0)),
        kind: ShapeKind::Media(MediaClip {
            media: video,
            video: true,
            autoplay: true,
            volume: 0.75,
            hide_while_not_playing: true,
            loop_play: true,
            rewind: true,
            play_across_slides: true,
            full_screen: true,
            ..Default::default()
        }),
        ..Default::default()
    });
    s0.shapes.push(deckcraft_model::Shape {
        id: aud_id,
        name: "ManualMutedAudio".into(),
        xfrm: Some(Xfrm::new(40.0, 160.0, 200.0, 100.0)),
        kind: ShapeKind::Media(MediaClip { media: audio, video: false, autoplay: false, volume: 0.0, loop_play: true, ..Default::default() }),
        ..Default::default()
    });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let back = deckcraft_pptx::import(&bytes).expect("import");
    let clips: Vec<&MediaClip> = back.slides[0]
        .shapes
        .iter()
        .filter_map(|s| match &s.kind {
            ShapeKind::Media(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(clips.len(), 2);
    assert!(
        clips[0].video
            && clips[0].autoplay
            && clips[0].hide_while_not_playing
            && clips[0].loop_play
            && clips[0].rewind
            && clips[0].play_across_slides
            && clips[0].full_screen
    );
    assert!((clips[0].volume - 0.75).abs() < 1e-4);
    assert!(
        !clips[1].video
            && !clips[1].autoplay
            && clips[1].loop_play
            && !clips[1].rewind
            && !clips[1].play_across_slides
            && !clips[1].hide_while_not_playing
            && !clips[1].full_screen
    );
    assert_eq!(clips[1].volume, 0.0);

    // Main-sequence automatic Play (mediacall) effect with indefinite media node also sets autoplay.
    let main_seq_play = self::clips(|v, _| {
        format!(
            r#"<p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst><p:par><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="indefinite"/><p:cond evt="onBegin" delay="0"><p:tn val="2"/></p:cond></p:stCondLst><p:childTnLst><p:par><p:cTn id="4" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="5" presetID="1" presetClass="mediacall" presetSubtype="0" fill="hold" nodeType="withEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:cmd type="call" cmd="playFrom(0.0)"><p:cBhvr><p:cTn id="6" dur="1" fill="hold"/><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cBhvr></p:cmd></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:seq><p:video><p:cMediaNode><p:cTn id="7" fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cMediaNode></p:video>"#
        )
    });
    assert!(main_seq_play[0].autoplay, "main-sequence automatic Play effect sets autoplay");

    // An `indefinite` + `onBegin delay="0"` condition whose `<p:tn>` references a different
    // click-dependent time node rather than the enclosing `mainSeq` must stay deferred.
    let non_main_seq_tn_play = self::clips(|v, _| {
        format!(
            r#"<p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst><p:par><p:cTn id="99" fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="100" fill="hold"><p:stCondLst><p:cond evt="onClick" delay="0"/></p:stCondLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par><p:par><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="indefinite"/><p:cond evt="onBegin" delay="0"><p:tn val="99"/></p:cond></p:stCondLst><p:childTnLst><p:par><p:cTn id="4" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="5" presetID="1" presetClass="mediacall" presetSubtype="0" fill="hold" nodeType="withEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:cmd type="call" cmd="playFrom(0.0)"><p:cBhvr><p:cTn id="6" dur="1" fill="hold"/><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cBhvr></p:cmd></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:seq><p:video><p:cMediaNode><p:cTn id="7" fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cMediaNode></p:video>"#
        )
    });
    assert!(!non_main_seq_tn_play[0].autoplay, "onBegin referencing non-mainSeq click-dependent node is not autoplay");

    // Command-based Play beneath an onClick or onEnd ancestor must not set autoplay.
    let deferred_cmd_play = self::clips(|v, a| {
        format!(
            r#"<p:par><p:cTn id="2" fill="hold"><p:stCondLst><p:cond evt="onClick" delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="3" presetID="1" presetClass="mediacall" presetSubtype="0" fill="hold" nodeType="withEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:cmd type="call" cmd="playFrom(0.0)"><p:cBhvr><p:cTn id="4" dur="1" fill="hold"/><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cBhvr></p:cmd></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par><p:par><p:cTn id="5" fill="hold"><p:stCondLst><p:cond evt="onEnd" delay="0"><p:tn val="2"/></p:cond></p:stCondLst><p:childTnLst><p:par><p:cTn id="6" presetID="1" presetClass="mediacall" presetSubtype="0" fill="hold" nodeType="withEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:cmd type="call" cmd="playFrom(0.0)"><p:cBhvr><p:cTn id="7" dur="1" fill="hold"/><p:tgtEl><p:spTgt spid="{a}"/></p:tgtEl></p:cBhvr></p:cmd></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par><p:video><p:cMediaNode><p:cTn id="8" fill="hold" display="0"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{v}"/></p:tgtEl></p:cMediaNode></p:video><p:audio><p:cMediaNode><p:cTn id="9" fill="hold" display="0"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{a}"/></p:tgtEl></p:cMediaNode></p:audio>"#
        )
    });
    assert!(!deferred_cmd_play[0].autoplay, "command-based Play under onClick ancestor is not autoplay");
    assert!(!deferred_cmd_play[1].autoplay, "command-based Play under onEnd ancestor is not autoplay");
}
