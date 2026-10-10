//! `wandur`: the desktop client.
//!
//! Usage: see [`USAGE`]. `WANDUR_RENDERER` is an alternative to `--renderer` and
//! `WANDUR_OUTPUT_FPS` to `--output-fps` (both for the perf harness). Values given on the command
//! line apply to this run and are not saved.

use wandur_app::sysstat::CountingAllocator;
use wandur_app::{Options, WandurApp};
use wandur_core::Endpoint;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const USAGE: &str = "wandur [HOST:PORT | tls://HOST:PORT]... [--connect ADDRESS] [--data-dir DIR] \
[--scrollback ROWS] [--font-size POINTS] [--output-fps N] [--theme NAME] [--skin Fleet|Armored|System] \
[--directory-url URL] \
[--show directory|settings|world:ID|unpinned:PANEL] [--window-size WxH] [--renderer wgpu|glow] \
[--scene NAME|list]
wandur --import-csharp DIR|wandur.db [--data-dir DIR] [--dry-run] [--include-passwords]

--scene NAME sets up a named screen in memory (nothing is saved). With WANDUR_SCREENSHOT=PATH.png
it is rendered headless at --window-size (default 1300x820, the C# reference size) and saved, then
the app exits; without it the scene opens in a window. --scene list prints the names.

--import-csharp brings the C# Wandur client's data (its data folder or its wandur.db, read only)
into this client's data directory, prints counts per kind and why anything was skipped, and
exits. --dry-run works on a copy and writes nothing. Saved passwords and agent API keys are
copied only with --include-passwords (the system may ask to allow access to each entry). Close
both clients first.";

struct Cli {
    options: Options,
    renderer: Option<String>,
    window: Option<[f32; 2]>,
    scene: Option<String>,
    import: Option<wandur_app::csharp_import::CliImport>,
}

fn parse_args() -> Result<Cli, String> {
    let mut options = Options::default();
    let mut renderer = None;
    let mut window = None;
    let mut scene = None;
    let mut import_source: Option<std::path::PathBuf> = None;
    let (mut dry_run, mut include_passwords) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--connect" => options.connect.push(parse_endpoint(&value("--connect")?)?),
            "--scrollback" => {
                options.scrollback = Some(
                    value("--scrollback")?
                        .parse()
                        .map_err(|_| "--scrollback needs a number of rows".to_string())?,
                );
            }
            "--output-fps" => {
                options.output_fps = Some(
                    value("--output-fps")?
                        .parse()
                        .map_err(|_| "--output-fps needs a number (0 for every frame)".to_string())?,
                );
            }
            "--font-size" => {
                options.font_size = Some(
                    value("--font-size")?
                        .parse()
                        .map_err(|_| "--font-size needs a number".to_string())?,
                );
            }
            "--renderer" => renderer = Some(value("--renderer")?),
            "--theme" => options.theme = Some(value("--theme")?),
            "--skin" => options.skin = Some(value("--skin")?),
            "--directory-url" => options.directory_url = Some(value("--directory-url")?),
            "--show" => options.show = Some(value("--show")?),
            "--window-size" => {
                let v = value("--window-size")?;
                let parsed = v
                    .split_once('x')
                    .and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?]));
                window = Some(parsed.ok_or("--window-size needs WIDTHxHEIGHT in points, such as 1200x780")?);
            }
            "--data-dir" => options.data_dir = Some(value("--data-dir")?.into()),
            "--scene" => scene = Some(value("--scene")?),
            "--import-csharp" => import_source = Some(value("--import-csharp")?.into()),
            "--dry-run" => dry_run = true,
            "--include-passwords" => include_passwords = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other if !other.starts_with('-') => options.connect.push(parse_endpoint(other)?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let import = match import_source {
        Some(source) => Some(wandur_app::csharp_import::CliImport {
            source,
            data_dir: options.data_dir.clone(),
            dry_run,
            include_passwords,
        }),
        None if dry_run || include_passwords => {
            return Err("--dry-run and --include-passwords go with --import-csharp".into());
        }
        None => None,
    };
    Ok(Cli {
        options,
        renderer,
        window,
        scene,
        import,
    })
}

/// The C# reference screenshots' window size.
const SCENE_SIZE: [f32; 2] = [1300.0, 820.0];

fn parse_endpoint(text: &str) -> Result<Endpoint, String> {
    text.parse().map_err(|e| format!("{text}: {e}"))
}

fn main() -> eframe::Result {
    wandur_app::sysstat::mark_main_started();
    let cli = match parse_args() {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("wandur: {e}");
            std::process::exit(2);
        }
    };
    let mut cli = cli;
    if let Some(import) = cli.import.take() {
        let from = wandur_core::import::csharp::secrets::csharp_vault();
        let to = wandur_core::login::vault::system();
        use wandur_app::csharp_import::{CliError, run_cli};
        match run_cli(&import, from.as_ref(), to.as_ref()) {
            Ok(summary) => {
                println!("{summary}");
                std::process::exit(0);
            }
            Err(CliError::NoDataDir) => {
                eprintln!("wandur: no data directory; pass --data-dir");
                std::process::exit(2);
            }
            Err(CliError::Import(e)) => {
                eprintln!("wandur: {e}");
                std::process::exit(1);
            }
        }
    }
    if let Some(name) = cli.scene.take() {
        if name == "list" {
            print!("{}", wandur_app::scene::list());
            std::process::exit(0);
        }
        let Some(scene) = wandur_app::scene::find(&name) else {
            eprintln!("wandur: no scene {name}; try --scene list");
            std::process::exit(2);
        };
        if let Some(path) = std::env::var_os("WANDUR_SCREENSHOT").filter(|p| !p.is_empty()) {
            let size = cli.window.unwrap_or(SCENE_SIZE);
            match wandur_app::scene::capture(scene, cli.options, size, std::path::Path::new(&path)) {
                Ok(warning) => {
                    if let Some(w) = warning {
                        eprintln!("wandur: {w}");
                    }
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("wandur: scene {name}: {e}");
                    std::process::exit(1);
                }
            }
        }
        wandur_app::scene::configure(scene, &mut cli.options);
        cli.window.get_or_insert(SCENE_SIZE);
    }
    // WANDUR_RENDERER lets the perf harness pick a renderer without passing flags.
    let chosen = cli.renderer.or_else(|| std::env::var("WANDUR_RENDERER").ok());
    let renderer = match chosen.as_deref() {
        None | Some("wgpu") => eframe::Renderer::Wgpu,
        #[cfg(feature = "glow")]
        Some("glow") => eframe::Renderer::Glow,
        Some(other) => {
            eprintln!("wandur: renderer {other} is not built in (try --features glow)");
            std::process::exit(2);
        }
    };
    // The skins draw into the caption area. On macOS the content runs under a transparent
    // title bar and the native traffic lights stay; elsewhere the app turns the OS frame off
    // for Fleet and Armored (and back on for System) once it has read the settings.
    let viewport = egui::ViewportBuilder::default()
        .with_title("Wandur")
        .with_inner_size(cli.window.unwrap_or([1200.0, 780.0]))
        .with_min_inner_size([480.0, 320.0]);
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    let native = eframe::NativeOptions {
        viewport,
        renderer,
        ..Default::default()
    };
    let mut options = cli.options;
    // The real window puts its menus where the platform expects them (macOS: the menu bar).
    options.native_menu = true;
    // Lets the perf harness compare redraw caps without passing flags.
    if let Some(fps) = std::env::var("WANDUR_OUTPUT_FPS").ok().and_then(|v| v.parse().ok()) {
        options.output_fps = Some(fps);
    }
    eframe::run_native(
        "Wandur",
        native,
        Box::new(move |cc| {
            options.renderer = renderer_info(cc);
            Ok(Box::new(WandurApp::new(&cc.egui_ctx, options)))
        }),
    )
}

/// The renderer in use and its graphics adapter, for Help > About Wandur's System information:
/// `wgpu, Metal, Apple M2 Pro` or `glow, <the GL renderer string>`.
fn renderer_info(cc: &eframe::CreationContext<'_>) -> Option<String> {
    if let Some(state) = &cc.wgpu_render_state {
        let info = state.adapter.get_info();
        return Some(format!("wgpu, {:?}, {}", info.backend, info.name));
    }
    #[cfg(feature = "glow")]
    if let Some(gl) = &cc.gl {
        use eframe::glow::HasContext as _;
        // SAFETY: the context is current while the app is created; RENDERER is a string query.
        let name = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
        return Some(format!("glow, {name}"));
    }
    None
}
