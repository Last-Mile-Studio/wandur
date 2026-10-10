//! The accessibility walk: every scene of `wandur --scene list` is set up in process against
//! loopback servers (The Lantern Road, Starfall Reach's login screen, the fixture directory
//! with an update to offer), run until it is ready, and its AccessKit tree is checked: every
//! control a person can click or focus has a name a screen reader can say.

use std::time::{Duration, Instant};

use wandur_app::Options;
use wandur_app::scene::{SCENES, audit};
use wandur_core::Endpoint;

use crate::directory_server::{DirectoryServer, latest_json};
use crate::mud_server::MudServer;

#[test]
#[ignore = "slow (about 25 seconds): run with --include-ignored, or the ship profile"]
fn every_scene_names_every_control() {
    let lantern = MudServer::start_lantern(0).unwrap();
    let (login, _log) = MudServer::start_login(0, false).unwrap();
    let directory = DirectoryServer::start_fixture(0).unwrap();
    directory.set_latest("200 OK", latest_json("0.1.6"));
    let lantern_at = Endpoint::new("127.0.0.1", lantern.port);
    let login_at = Endpoint::new("127.0.0.1", login.port);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.superpowers/test-data")
        .join(format!("a11y-{}", std::process::id()));
    let started = Instant::now();
    // A few scenes at a time; each has its own egui context and app.
    let chunks: Vec<Vec<&'static wandur_app::scene::Scene>> = {
        let threads = 6;
        let mut out = vec![Vec::new(); threads];
        for (i, scene) in SCENES.iter().enumerate() {
            out[i % threads].push(scene);
        }
        out
    };
    let results: Vec<(String, usize, Vec<String>, Option<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let (lantern_at, login_at, root) = (lantern_at.clone(), login_at.clone(), root.clone());
                let directory = directory.address();
                scope.spawn(move || {
                    let mut out = Vec::new();
                    for scene in chunk {
                        let data = root.join(scene.name);
                        let _ = std::fs::remove_dir_all(&data);
                        let options = Options {
                            data_dir: Some(data.clone()),
                            // The Lantern Road stands in for every world (also Legends of the
                            // Jedi: the walk needs its controls, not its room).
                            connect: vec![lantern_at.clone(), login_at.clone()],
                            directory_url: Some(directory.clone()),
                            ..Default::default()
                        };
                        let (nodes, unnamed, warning) = audit(scene, options, [1300.0, 820.0], Duration::from_secs(30));
                        out.push((
                            scene.name.to_string(),
                            nodes,
                            unnamed.iter().map(ToString::to_string).collect(),
                            warning,
                        ));
                        let _ = std::fs::remove_dir_all(&data);
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(results.len(), SCENES.len());
    let mut problems = Vec::new();
    for (name, nodes, unnamed, warning) in &results {
        if let Some(w) = warning {
            eprintln!("{w}");
        }
        assert!(*nodes > 30, "{name}: only {nodes} AccessKit nodes");
        for u in unnamed {
            problems.push(format!("{name}: {u}"));
        }
    }
    eprintln!(
        "walked {} scenes ({} nodes) in {:?}",
        results.len(),
        results.iter().map(|r| r.1).sum::<usize>(),
        started.elapsed()
    );
    assert!(problems.is_empty(), "controls without a name:\n{}", problems.join("\n"));
}
