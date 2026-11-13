use super::{fixture, terminal};
use factory_core::*;
use sha2::{Digest, Sha256};

const SCRIPT: &str = r#"#!/usr/bin/env python3
import hashlib, json, os, pathlib, sys
scenario = SCENARIO
prompt = sys.stdin.read()
session = pathlib.Path(os.environ['AI_FACTORY_RESULT_PATH']).parent
node = os.environ['AI_FACTORY_NODE_ID']
memory = pathlib.Path(os.environ['AI_FACTORY_MEMORY_DIR'])
workspace = pathlib.Path.cwd()
count = session / (node + '.count')
attempt = int(count.read_text()) + 1 if count.exists() else 1
count.write_text(str(attempt))
def emit(value):
    text = value if isinstance(value, str) else json.dumps(value)
    print(json.dumps({'type': 'text', 'part': {'text': text}}))
if node == 'consumer':
    dependencies = json.loads(prompt.split('Dependency results:\n', 1)[1].split('\n\nWorkspace:', 1)[0])
    artifacts = dependencies[0]['result']['artifacts']
    assert len(artifacts) == 5, artifacts
    for artifact in artifacts:
        path = pathlib.Path(artifact['path'])
        assert path.is_file() and path.is_relative_to(memory)
        assert hashlib.sha256(path.read_bytes()).hexdigest() == artifact['sha256']
    (memory / 'consumer-ran').write_text('all artifacts verified')
    emit({'status': 'succeeded', 'summary': 'consumed verified files', 'artifacts': [], 'data': {}})
    sys.exit(0)
manifest = session / 'original-artifacts.json'
if attempt == 1:
    (session / 'work-executions').write_text('1')
    base = memory if scenario != 'workspace' else workspace
    folder = base / 'artifacts/builder'
    folder.mkdir(parents=True)
    files = {'builder-report.md': 'report', 'implementation.patch': 'patch',
             'validation/stdout.txt': 'tests passed', 'validation/nested/stderr.txt': '',
             'SHA256SUMS.txt': 'checksums'}
    for name, text in files.items():
        path = folder / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    def ref(name, hashed=True):
        path = folder / name
        return {'path': str(path.relative_to(base)), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest() if hashed else '', 'kind': 'artifact'}
    artifacts = [ref('builder-report.md'), ref('implementation.patch'),
                 ref('validation', False), ref('SHA256SUMS.txt', False)]
    if scenario == 'absolute':
        for artifact in artifacts:
            artifact['path'] = str(base / artifact['path'])
    elif scenario == 'workspace_prefix_file':
        (workspace / 'artifacts').write_text('not a directory')
    elif scenario == 'ambiguous':
        other = workspace / artifacts[0]['path']
        other.parent.mkdir(parents=True)
        other.write_text('other report')
    elif scenario == 'mismatch':
        artifacts[0]['sha256'] = '0' * 64
    elif scenario == 'directory_hash':
        artifacts[2]['sha256'] = '0' * 64
    elif scenario == 'missing':
        artifacts[0]['path'] = 'missing-report.md'
    elif scenario == 'empty_directory':
        empty = folder / 'empty'
        empty.mkdir()
        artifacts = [ref('empty', False)]
    elif scenario == 'outside':
        artifacts[0]['path'] = str(workspace.parent.parent / 'outside-report.txt')
        pathlib.Path(artifacts[0]['path']).write_text('outside')
        artifacts[0]['sha256'] = ''
    elif scenario == 'traversal':
        artifacts[0]['path'] = '../' + memory.name + '/artifacts/builder/builder-report.md'
    elif scenario == 'special_file':
        os.mkfifo(folder / 'validation/fifo')
    elif scenario == 'symlink':
        (folder / 'validation/link.txt').symlink_to(folder / 'builder-report.md')
    elif scenario == 'parent_symlink':
        link = memory / 'linked'
        link.symlink_to(folder, target_is_directory=True)
        artifacts[0]['path'] = 'linked/builder-report.md'
    manifest.write_text(json.dumps(artifacts))
    if scenario == 'prose':
        emit('Implementation complete. Artifacts in memory: artifacts/builder/builder-report.md, implementation.patch, validation/, SHA256SUMS.txt. No GUI verification.')
        sys.exit(0)
artifacts = json.loads(manifest.read_text())
emit({'status': 'succeeded', 'summary': 'Implementation complete; GUI not verified.',
      'artifacts': artifacts, 'data': {'native_smoke': 'not_run_GUI_prohibited'}})
"#;

async fn delivery(
    scenario: &str,
    agent: &str,
    consumer: bool,
) -> (tempfile::TempDir, String, RunDetail) {
    let (dir, factory) = fixture("exit 1", 4);
    std::fs::write(
        dir.path().join("fake-agent"),
        SCRIPT.replace("SCENARIO", &format!("{scenario:?}")),
    )
    .unwrap();
    let mut settings = factory.settings().unwrap();
    settings.flows[0].nodes[0].agent = agent.into();
    if consumer {
        let mut next = settings.flows[0].nodes[0].clone();
        next.id = "consumer".into();
        next.name = "consumer".into();
        next.depends_on = vec![settings.flows[0].nodes[0].id.clone()];
        settings.flows[0].nodes.push(next);
    }
    factory.save_settings(settings).unwrap();
    let id = factory
        .enqueue(TriggerEvent::issue(
            "acme/repo",
            8,
            "[AI] window size and pos",
            "alice",
            "",
        ))
        .unwrap()
        .unwrap();
    factory.start();
    let detail = terminal(&factory, &id).await;
    factory.shutdown().await;
    (dir, id, detail)
}

#[tokio::test]
async fn memory_artifact_delivery_reaches_downstream_on_first_response() {
    for agent in ["codex", "opencode"] {
        for scenario in ["memory", "workspace", "absolute", "workspace_prefix_file"] {
            let (dir, id, detail) = delivery(scenario, agent, true).await;
            assert_eq!(
                detail.run.status, "succeeded",
                "{agent}/{scenario}: {:?}",
                detail.nodes
            );
            assert!(detail.nodes.iter().all(|node| node.status == "succeeded"));
            let session = dir.path().join("data/sessions").join(&id);
            assert_eq!(
                std::fs::read_to_string(session.join("work-executions")).unwrap(),
                "1"
            );
            assert_eq!(
                std::fs::read_to_string(session.join("spec.count")).unwrap(),
                "1"
            );
            assert!(!dir.path().join("data/workspaces").join(&id).exists());
            let result = detail
                .nodes
                .iter()
                .find(|node| node.id == "spec")
                .unwrap()
                .result
                .as_ref()
                .unwrap();
            assert_eq!(result["data"]["native_smoke"], "not_run_GUI_prohibited");
            let artifacts = result["artifacts"].as_array().unwrap();
            assert_eq!(artifacts.len(), 5);
            let mut contents: Vec<_> = artifacts
                .iter()
                .map(|artifact| {
                    let path = std::path::Path::new(artifact["path"].as_str().unwrap());
                    let bytes = std::fs::read(path).unwrap();
                    assert_eq!(artifact["sha256"], format!("{:x}", Sha256::digest(&bytes)));
                    assert!(path
                        .ancestors()
                        .any(|parent| parent.join("consumer-ran").is_file()));
                    bytes
                })
                .collect();
            contents.sort();
            let mut expected = vec![
                b"report".to_vec(),
                b"patch".to_vec(),
                b"tests passed".to_vec(),
                vec![],
                b"checksums".to_vec(),
            ];
            expected.sort();
            assert_eq!(contents, expected);
        }
    }
}

#[tokio::test]
async fn artifact_delivery_rejects_unsafe_ambiguous_or_unverifiable_references() {
    for (scenario, diagnostic) in [
        ("ambiguous", "Ambiguous artifact path"),
        ("mismatch", "Artifact hash mismatch"),
        (
            "directory_hash",
            "Directory artifact cannot declare a file hash",
        ),
        ("missing", "Artifact does not exist"),
        ("empty_directory", "Artifact directory contains no files"),
        ("outside", "Artifact escaped authorized workspace/memory"),
        ("traversal", "Artifact parent traversal forbidden"),
        ("special_file", "Artifact must be a regular file"),
        ("symlink", "Artifact symlink forbidden"),
        ("parent_symlink", "Artifact symlink forbidden"),
    ] {
        let (dir, id, detail) = delivery(scenario, "opencode", true).await;
        assert_eq!(
            detail.run.status, "failed",
            "{scenario}: {:?}",
            detail.nodes
        );
        let node = detail.nodes.iter().find(|node| node.id == "spec").unwrap();
        assert!(
            node.error.as_deref().unwrap().contains(diagnostic),
            "{scenario}: {:?}",
            node.error
        );
        assert_eq!(
            detail
                .nodes
                .iter()
                .find(|node| node.id == "consumer")
                .unwrap()
                .status,
            "skipped"
        );
        assert_eq!(
            std::fs::read_to_string(
                dir.path()
                    .join("data/sessions")
                    .join(&id)
                    .join("spec.count")
            )
            .unwrap(),
            "1"
        );
        let archive = dir
            .path()
            .join("data/memory/acme/repo/runs")
            .join(&id)
            .join("artifacts/spec");
        assert!(
            !archive.exists() || std::fs::read_dir(archive).unwrap().next().is_none(),
            "{scenario}: invalid archive copy retained"
        );
    }
}

#[tokio::test]
async fn prose_artifact_delivery_fails_once_and_preserves_original_memory_files() {
    for agent in ["codex", "opencode"] {
        let (dir, id, detail) = delivery("prose", agent, true).await;
        assert_eq!(detail.run.status, "failed");
        let session = dir.path().join("data/sessions").join(&id);
        assert_eq!(
            std::fs::read_to_string(session.join("spec.count")).unwrap(),
            "1"
        );
        assert!(!session.join("consumer.count").exists());
        assert!(session.join("spec.invalid-response.txt").exists());
        let memory = dir.path().join("data/memory/acme/repo/runs").join(&id);
        assert_eq!(
            std::fs::read_to_string(memory.join("artifacts/builder/builder-report.md")).unwrap(),
            "report"
        );
        assert_eq!(
            std::fs::read_to_string(memory.join("artifacts/builder/implementation.patch")).unwrap(),
            "patch"
        );
        assert!(!dir.path().join("data/workspaces").join(&id).exists());
    }
}
