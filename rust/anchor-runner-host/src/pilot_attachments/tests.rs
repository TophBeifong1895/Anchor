use super::*;
use anchor_platform_session::{CreateSession, SessionError};
use std::{fs, io::Cursor};

struct Fixture {
    _root: tempfile::TempDir,
    data: PathBuf,
    work: PathBuf,
    sessions: SessionStore,
    session: Session,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("state");
        let work = root.path().join("work");
        fs::create_dir_all(data.join("platform")).unwrap();
        let sessions = SessionStore::open(data.join("platform/sessions.sqlite")).unwrap();
        let session = sessions
            .create(
                "owner",
                CreateSession {
                    id: Some("pilot".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        crate::pilot_resources::register(&data, &work, "owner", &session).unwrap();
        Self {
            _root: root,
            data,
            work,
            sessions,
            session,
        }
    }
    fn input(
        &self,
        request: &str,
        prompt: Option<&str>,
        prepared: &PreparedAttachments,
        source: Option<&str>,
    ) -> (Turn, bool) {
        let trusted = source
            .and_then(|source| {
                self.sessions
                    .get_turn("owner", &self.session.id, source)
                    .ok()
            })
            .and_then(|source| {
                source_inputs(&self.data, &self.sessions, "owner", &self.session, &source)
                    .unwrap()
                    .1
            });
        self.sessions
            .create_turn_with_attachments(
                "owner",
                &self.session.id,
                request,
                prompt,
                &identity(prepared, trusted.as_ref()),
                |turn| {
                    freeze_inputs(
                        &self.data,
                        "owner",
                        &self.session,
                        turn,
                        trusted.as_ref(),
                        prepared,
                    )
                    .map_err(SessionError::Storage)
                },
            )
            .unwrap()
    }
    fn finish(&self, turn: &Turn) {
        self.sessions
            .finish_turn(
                "owner",
                &self.session.id,
                &turn.id,
                TurnStatus::Completed,
                None,
            )
            .unwrap();
    }
}
fn file(name: &str, bytes: &[u8]) -> PreparedAttachments {
    attachments::from_bytes(name.into(), bytes.to_vec(), None).unwrap()
}
fn png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
        2,
        1,
        image::Rgb([0, 128, 0]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    bytes.into_inner()
}

#[test]
fn input_admission_is_atomic_and_request_identity_covers_bytes_name_mime_and_source() {
    let f = Fixture::new();
    let prepared = file("notes.txt", b"actual document");
    let (turn, created) = f.input("request", None, &prepared, None);
    assert!(created);
    assert!(turn.prompt.is_none());
    let refs = input_references(&f.sessions, "owner", "pilot", &turn.id).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(
        read(
            &f.data,
            &f.sessions,
            "owner",
            &f.session,
            &turn,
            &refs[0].id
        )
        .unwrap()
        .1,
        b"actual document"
    );
    assert_eq!(
        readonly_inputs(&f.data, &f.sessions, "owner", &f.session, &turn).unwrap()[0].destination,
        PathBuf::from("/in/pilot")
    );
    let retry = f
        .sessions
        .create_turn_with_attachments(
            "owner",
            "pilot",
            "request",
            None,
            &identity(&prepared, None),
            |_| panic!("retry must not freeze or execute again"),
        )
        .unwrap();
    assert_eq!(retry, (turn.clone(), false));
    for changed in [
        file("notes.txt", b"changed document"),
        file("renamed.txt", b"actual document"),
        attachments::from_bytes(
            "notes.txt".into(),
            b"actual document".to_vec(),
            Some("text/plain"),
        )
        .unwrap(),
    ] {
        assert!(matches!(
            f.sessions.create_turn_with_attachments(
                "owner",
                "pilot",
                "request",
                None,
                &identity(&changed, None),
                |_| Ok(json!([]))
            ),
            Err(SessionError::Conflict(_))
        ));
    }
    assert!(matches!(
        f.sessions.create_turn_with_attachments(
            "owner",
            "pilot",
            "request",
            None,
            &json!({"attachments":prepared.manifest(),"source_turn":"another-turn"}),
            |_| Ok(json!([]))
        ),
        Err(SessionError::Conflict(_))
    ));
    f.finish(&turn);
    assert!(
        f.sessions
            .create_turn_with_attachments(
                "owner",
                "pilot",
                "failed",
                Some("document"),
                &identity(&prepared, None),
                |_| Err(SessionError::Storage("freeze failed".into()))
            )
            .is_err()
    );
    assert_eq!(f.sessions.list_turns("owner", "pilot").unwrap().len(), 1);
    assert!(input_references(&f.sessions, "other", "pilot", &turn.id).is_err());
    let mut recreated = f.session.clone();
    recreated.created_at += chrono::Duration::seconds(1);
    assert!(
        read(
            &f.data,
            &f.sessions,
            "owner",
            &recreated,
            &turn,
            &refs[0].id
        )
        .is_err()
    );
    let input = directory(&f.data, "owner", &f.session, &turn)
        .unwrap()
        .join("input/files/notes.txt");
    fs::remove_file(input).unwrap();
    assert!(
        read(
            &f.data,
            &f.sessions,
            "owner",
            &f.session,
            &turn,
            &refs[0].id
        )
        .is_err()
    );
}

#[test]
fn published_files_are_immutable_bounded_replayable_and_removed_by_existing_lifecycle() {
    let f = Fixture::new();
    let (turn, _) = f.input(
        "request",
        Some("make a report"),
        &PreparedAttachments::default(),
        None,
    );
    let workspace = crate::pilot_tools::session_workspace(&f.work, "owner", "pilot");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("report.txt"), b"original report").unwrap();
    let output = publish_workspace_file(
        &f.data,
        &f.sessions,
        "owner",
        "pilot",
        &turn.id,
        &workspace,
        "report.txt",
        Some("report final.txt"),
    )
    .unwrap();
    fs::write(workspace.join("report.txt"), b"changed workspace").unwrap();
    assert_eq!(
        read(&f.data, &f.sessions, "owner", &f.session, &turn, &output.id)
            .unwrap()
            .1,
        b"original report"
    );
    let image = publish(
        &f.data,
        &f.sessions,
        "owner",
        "pilot",
        &turn.id,
        file("chart.png", &png()),
    )
    .unwrap();
    assert_eq!(image.media_type.as_deref(), Some("image/png"));
    assert_eq!(
        publish(
            &f.data,
            &f.sessions,
            "owner",
            "pilot",
            &turn.id,
            file("chart.png", &png())
        )
        .unwrap(),
        image
    );
    assert!(output.url.starts_with(&format!(
        "/sessions/pilot/turns/{}/attachments/output-",
        turn.id
    )));
    assert_eq!(output.download_url, format!("{}?download=1", output.url));
    assert!(
        !serde_json::to_string(&output)
            .unwrap()
            .contains(f.data.to_str().unwrap())
    );
    assert_eq!(
        f.sessions
            .turn_events("owner", "pilot", &turn.id, 0)
            .unwrap()
            .iter()
            .filter(|e| e.data["type"] == "attachment")
            .count(),
        2
    );
    for index in 2..16 {
        publish(
            &f.data,
            &f.sessions,
            "owner",
            "pilot",
            &turn.id,
            file(&format!("file-{index}"), b"small"),
        )
        .unwrap();
    }
    assert!(
        publish(
            &f.data,
            &f.sessions,
            "owner",
            "pilot",
            &turn.id,
            file("seventeenth", b"small")
        )
        .is_err()
    );
    f.finish(&turn);
    assert!(
        publish(
            &f.data,
            &f.sessions,
            "owner",
            "pilot",
            &turn.id,
            file("late", b"small")
        )
        .is_err()
    );
    let reopened = SessionStore::open(f.data.join("platform/sessions.sqlite")).unwrap();
    assert_eq!(
        read(&f.data, &reopened, "owner", &f.session, &turn, &image.id)
            .unwrap()
            .1,
        png()
    );
    let history = crate::goose_acp::pilot::messages(&reopened, "owner", "pilot").unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|message| message["attachments"].is_array())
            .count(),
        16
    );
    assert!(history.iter().all(|message| message["turn"] == turn.id));
    crate::pilot_resources::delete(&f.sessions, &f.data, &f.work, "owner", &f.session).unwrap();
    assert!(!crate::pilot_host::scope(&f.data, "owner", &f.session).exists());
    assert!(!workspace.exists());
}

#[test]
fn publishing_reads_only_regular_workspace_files_and_input_scope_selects_one_turn() {
    let f = Fixture::new();
    let (first, _) = f.input(
        "first",
        Some("first task"),
        &file("first.txt", b"first"),
        None,
    );
    f.finish(&first);
    let inherited = prepared_inputs(&f.data, &f.sessions, "owner", &f.session, &first).unwrap();
    let (next, _) = f.input("resume", None, &inherited, Some(&first.id));
    assert_eq!(
        input_references(&f.sessions, "owner", "pilot", &next.id).unwrap()[0].name,
        "first.txt"
    );
    let record = load_record(
        &directory(&f.data, "owner", &f.session, &next)
            .unwrap()
            .join("input"),
        "owner",
        &f.session,
        &next,
        "input",
    )
    .unwrap();
    assert_eq!(record.source_turn.as_deref(), Some(first.id.as_str()));
    assert_eq!(record.files_turn.as_deref(), Some(first.id.as_str()));
    let inherited_files = directory(&f.data, "owner", &f.session, &next)
        .unwrap()
        .join("input/files");
    assert_eq!(fs::read_dir(inherited_files).unwrap().count(), 0);
    assert_eq!(
        input_references(&f.sessions, "owner", "pilot", &next.id).unwrap(),
        input_references(&f.sessions, "owner", "pilot", &first.id).unwrap()
    );
    assert_eq!(
        read(&f.data, &f.sessions, "owner", &f.session, &next, "input-0")
            .unwrap()
            .1,
        b"first"
    );
    assert_eq!(
        readonly_inputs(&f.data, &f.sessions, "owner", &f.session, &next).unwrap()[0].source,
        directory(&f.data, "owner", &f.session, &first)
            .unwrap()
            .join("input/files")
    );
    assert_eq!(
        crate::goose_acp::pilot::messages(&f.sessions, "owner", "pilot")
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "user")
            .count(),
        1
    );
    f.finish(&next);
    let (continued, _) = f.input(
        &format!("continuation:{}", next.id),
        Some("Host applied configuration"),
        &inherited,
        Some(&next.id),
    );
    let continued_record = load_record(
        &directory(&f.data, "owner", &f.session, &continued)
            .unwrap()
            .join("input"),
        "owner",
        &f.session,
        &continued,
        "input",
    )
    .unwrap();
    assert_eq!(
        continued_record.source_turn.as_deref(),
        Some(next.id.as_str())
    );
    assert_eq!(
        continued_record.files_turn.as_deref(),
        Some(first.id.as_str())
    );
    assert_eq!(
        crate::goose_acp::pilot::messages(&f.sessions, "owner", "pilot")
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "user")
            .count(),
        1
    );
    f.finish(&continued);
    let (fresh, _) = f.input("fresh", None, &file("fresh.txt", b"fresh"), None);
    let mounted = readonly_inputs(&f.data, &f.sessions, "owner", &f.session, &fresh).unwrap();
    assert_eq!(mounted.len(), 1);
    assert!(!mounted[0].source.join("first.txt").exists());
    let workspace = crate::pilot_tools::session_workspace(&f.work, "owner", "pilot");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("normal"), "normal").unwrap();
    std::os::unix::fs::symlink(workspace.join("normal"), workspace.join("link")).unwrap();
    for path in ["link", "../normal", "/etc/hosts", "."] {
        assert!(
            publish_workspace_file(
                &f.data,
                &f.sessions,
                "owner",
                "pilot",
                &fresh.id,
                &workspace,
                path,
                None
            )
            .is_err()
        );
    }
    assert!(
        publish_workspace_file(
            &f.data,
            &f.sessions,
            "owner",
            "pilot",
            &fresh.id,
            &workspace,
            "normal",
            Some("../bad")
        )
        .is_err()
    );
}
