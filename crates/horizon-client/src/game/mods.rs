//! Horizon mods in the game (`docs/mods.md`): loading the ones turned on,
//! their tick, frame, chat and command hooks, their HUD panels and what
//! they print. With no mod on, `horizon_mods::Mods` holds no engine and
//! every hook here returns after one check.
//!
//! Code mods run only in builds with the `mods` feature (release builds):
//! without it the game links no wasmtime, and every hook does nothing.
//! Content mods (`content_root`) load in every build.
use super::Game;
use horizon_client::{
    command::{Dispatcher, vanilla},
    extras_hud::Corners,
    hud::HudBatch,
    mod_list,
};
use std::{path::PathBuf, time::Duration};
#[cfg(feature = "mods")]
use {
    super::Screen,
    super::{execution_role::ExecutionRole, lan::LanHost, server_mods::ModEvent},
    horizon_client::{
        extras::Corner,
        extras_hud,
        items::ItemStack,
        keybinds,
        mod_list::Installed,
        options_ui::Page,
        text::{self, Text},
        ui,
    },
    horizon_mods::{
        Clock, ModKey, Mods, MotionState, MotionVec, Output, Player, Runs, ServerPlayer, Snapshot,
        Status, UiScreen, UiWidget,
    },
    std::time::Instant,
    winit::{event::MouseButton, keyboard::KeyCode},
};

#[derive(Default)]
pub(super) struct ModsState {
    #[cfg(feature = "mods")]
    pub(super) mods: Mods,
    /// Capability `input`: a claimed item's use while the button is down.
    #[cfg(feature = "mods")]
    pub(super) using: Option<super::mod_input::ModUse>,
    /// Capability `render`'s camera shakes.
    #[cfg(feature = "mods")]
    pub(super) shake: super::mod_render::Shake,
    /// `--dev-mod PATH`: always on, reloaded when rebuilt.
    pub(super) dev: Vec<PathBuf>,
    /// When dev mods were last checked for a rebuild.
    #[cfg(feature = "mods")]
    checked: Option<Instant>,
    /// The chat's command tree while a running mod owns commands.
    #[cfg(feature = "mods")]
    tree: Option<Dispatcher>,
    /// A server mod runs here: what players do is queued for it.
    #[cfg(feature = "mods")]
    pub(super) server: bool,
    /// The players server mods saw last tick, for joins and leaves.
    #[cfg(feature = "mods")]
    pub(super) players: Vec<ServerPlayer>,
    /// What players did this tick, for server mods at its end.
    #[cfg(feature = "mods")]
    pub(super) events: Vec<ModEvent>,
    /// A running server mod has capability `world`: its calls get the
    /// levels' chunks to read.
    #[cfg(feature = "mods")]
    pub(super) world: bool,
    /// Blocks server mods set, by dimension, made before that level's next
    /// tick.
    #[cfg(feature = "mods")]
    pub(super) block_writes: Vec<(String, horizon_mods::BlockChange)>,
    /// `command.run` lines for this window's own world, run on the next tick:
    /// run at once, their chat lines would reach the mods while their output
    /// is still being shown, and a mod answering its own lines would recurse.
    #[cfg(feature = "mods")]
    commands: Vec<String>,
    /// `command.run-quiet` batches for this window's own world, by mod id,
    /// run on the next tick as `commands` are.
    #[cfg(feature = "mods")]
    quiet: Vec<(String, Vec<String>)>,
    /// The server's command tree with the running mods' commands added.
    #[cfg(feature = "mods")]
    remote_tree: Option<Dispatcher>,
    /// A `ui` mod's screen or settings page while it is up.
    #[cfg(feature = "mods")]
    screen: Option<ModScreen>,
    /// The HUD font's advances for `render.text-width`, made once.
    #[cfg(feature = "mods")]
    font_widths: std::sync::Arc<[f32]>,
    /// Capability `recipes`'s tables, made once a mod reads them.
    #[cfg(feature = "mods")]
    pub(super) recipes: Option<std::sync::Arc<horizon_mods::Recipes>>,
    /// The recipe book's known displays, and the book's version they are of.
    #[cfg(feature = "mods")]
    pub(super) known_recipes: (Option<u64>, std::sync::Arc<[i32]>),
    /// Capability `ui`'s item panel beside container screens.
    #[cfg(feature = "mods")]
    pub(super) browser: super::mod_browser::Browser,
    /// Extras that are official mods now, being installed.
    #[cfg(feature = "mods")]
    pub(super) moved: super::moved_extras::MovedState,
    /// Playing at the last frame, for `mod_input_frame`.
    #[cfg(feature = "mods")]
    pub(super) playing: bool,
    /// A `motion` mod forgot the fall this tick: the player's own world's
    /// server hears the player as grounded, so no fall builds up.
    pub(super) safe_fall: bool,
    /// A `motion` mod's pose for the player's model, last tick's and this
    /// tick's, blended in between when drawn.
    pub(super) pose: [Option<horizon_client::mobs::ModPose>; 2],
    /// A `motion` mod's wind volume, and whether its loop is playing.
    pub(super) wind: Option<f32>,
    pub(super) wind_playing: bool,
    /// A `motion` mod's camera.
    #[cfg(feature = "mods")]
    pub(super) camera: Option<horizon_mods::ModCamera>,
    /// The chase camera's eased distance back and side offset, and when
    /// they were last eased: only the mouse turns the camera at once.
    #[cfg(feature = "mods")]
    pub(super) cam_ease: std::cell::Cell<Option<(f64, f64, std::time::Instant)>>,
    /// The wall turn eased per frame (the mod steps it per tick: 20 steps a
    /// second read as a staircase), and when it was last eased.
    #[cfg(feature = "mods")]
    pub(super) wall_ease: std::cell::Cell<(f64, Option<std::time::Instant>)>,
}

/// The mappings `input.mapping` answers for, with the default key their
/// binding is translated to (none: a mouse button, attack or use).
#[cfg(feature = "mods")]
const MAPPINGS: [(&str, Option<winit::keyboard::KeyCode>); 9] = {
    use winit::keyboard::KeyCode as K;
    [
        ("key.forward", Some(K::KeyW)),
        ("key.left", Some(K::KeyA)),
        ("key.back", Some(K::KeyS)),
        ("key.right", Some(K::KeyD)),
        ("key.jump", Some(K::Space)),
        ("key.sneak", Some(K::ShiftLeft)),
        ("key.sprint", Some(K::ControlLeft)),
        ("key.attack", None),
        ("key.use", None),
    ]
};
/// Characters `render.text-width` knows exactly: U+0000 to U+024F.
#[cfg(feature = "mods")]
const FONT_CHARS: u32 = 0x250;

/// A `ui` mod's own screen, or its settings page from Options > Mods: the
/// widgets the mod pushed, laid out and drawn with the menus' own widgets.
#[cfg(feature = "mods")]
struct ModScreen {
    /// The running mod's index in `mods.mods.entries()`.
    owner: usize,
    /// Opened from Options > Mods: Done and Esc return there.
    settings: bool,
    /// What the mod's screen replaced (Playing, Chat, ...), to restore.
    under: Screen,
    /// One scroll per `list` widget, in widget order.
    scrolls: Vec<ui::Scroll>,
    /// A key row waiting for the key to bind: the mod key's name.
    binding: Option<String>,
}
impl ModsState {
    /// The chat's command tree: vanilla's, plus the running mods' commands.
    pub(super) fn dispatcher(&self) -> &Dispatcher {
        #[cfg(feature = "mods")]
        if let Some(tree) = &self.tree {
            return tree;
        }
        vanilla::game_dispatcher()
    }
    /// The server's tree plus the mods' commands, when a mod owns any.
    pub(super) fn remote_dispatcher(&self) -> Option<&Dispatcher> {
        #[cfg(feature = "mods")]
        if let Some(tree) = &self.remote_tree {
            return Some(tree);
        }
        None
    }
    /// Whether a mod runs: the chat keeps its lines for them.
    pub(super) fn running(&self) -> bool {
        #[cfg(feature = "mods")]
        if self.mods.is_running() {
            return true;
        }
        false
    }
    /// Whether a mod draws a HUD panel or a corner panel.
    pub(super) fn has_panels(&self) -> bool {
        #[cfg(feature = "mods")]
        if self.mods.panels().next().is_some() || self.mods.corner_panels().next().is_some() {
            return true;
        }
        false
    }
}
/// How often dev mods are checked for a rebuild.
#[cfg(feature = "mods")]
const RELOAD_CHECK: Duration = Duration::from_secs(1);

/// A vanilla command name: mods may not take it.
#[cfg(feature = "mods")]
fn vanilla_command(name: &str) -> bool {
    let d = vanilla::game_dispatcher();
    d.nodes[0]
        .children
        .iter()
        .any(|&c| d.nodes[c].name() == name)
}

/// `root` with the content mods that are on layered over it
/// (docs/modpacks.md): their blocks, items, data and assets. Read once at
/// start, before the resources load; a mod whose content is refused is left
/// out here and its row on the Mods page says why.
pub(super) fn content_root(
    root: &std::path::Path,
    enabled: &std::collections::BTreeSet<String>,
    dev: &[std::path::PathBuf],
) -> std::path::PathBuf {
    use horizon_client::mod_content::{self, ContentMod};
    use horizon_mods::{Capability, Content, Package};
    let here = std::env::current_dir().unwrap_or_default();
    let mut paths = horizon_mods::find(&here.join(mod_list::DIR));
    paths.retain(|p| !dev.contains(p));
    paths.extend(dev.iter().cloned());
    let mut mods: Vec<ContentMod> = Vec::new();
    for path in &paths {
        let Ok(package) = Package::read(path) else {
            continue;
        };
        let m = &package.manifest;
        let on = enabled.contains(&m.id) || dev.contains(path);
        if !on
            || !m.capabilities.contains(&Capability::Registries)
            || mods.iter().any(|c| c.id == m.id)
        {
            continue;
        }
        let content = Content::read(path, m).and_then(|content| {
            // The first of two mods owning one namespace stays on.
            let own = (m.id.as_str(), content.namespaces.as_slice());
            match mods
                .iter()
                .find_map(|c| horizon_mods::content::clash((&c.id, &c.content.namespaces), own))
            {
                Some(e) => Err(e),
                None => Ok(content),
            }
        });
        match content {
            Ok(content) => mods.push(ContentMod {
                id: m.id.clone(),
                content,
            }),
            Err(e) => log::warn!("Horizon: mod {} content not loaded: {e}", m.id),
        }
    }
    match mod_content::resource_root(root, &mods, &here.join(".horizon-cache")) {
        Ok(out) => {
            if out != root {
                log::info!(
                    "Horizon: content from {} mod(s) in {}",
                    mods.len(),
                    out.display()
                );
            }
            out
        }
        Err(e) => {
            log::warn!("Horizon: content mods not loaded: {e}");
            root.to_owned()
        }
    }
}

/// A build without the `mods` feature: no mod runs.
#[cfg(not(feature = "mods"))]
impl Game<'_> {
    /// Says why the installed and dev mods don't run.
    pub(super) fn start_mods(&mut self) {
        let dir = Self::options_path().with_file_name(mod_list::DIR);
        let found = horizon_mods::find(&dir).len() + self.mods.dev.len();
        if found > 0 {
            log::warn!(
                "Horizon: {found} mod(s) not started: this build has no mod runtime (build with --features mods)"
            );
        }
    }
    pub(super) fn merge_remote_tree(&mut self) {}
    pub(super) fn mod_key_rows(&self) -> Vec<horizon_client::options_ui::ModKeyRow> {
        Vec::new()
    }
    pub(super) fn apply_mod_keys(&mut self) {}
    pub(super) fn sync_mods(&mut self) {}
    pub(super) fn tick_mods(&mut self) {}
    pub(super) fn mods_frame(&mut self, _elapsed: Duration) {}
    pub(super) fn mod_command(&mut self, _command: &str) -> bool {
        false
    }
    pub(super) fn mods_chat(&mut self, _line: &str) {}
    pub(super) fn draw_mod_panels(&self, _batch: &mut HudBatch, _corners: &mut Corners) {}
    pub(super) fn open_mod_screen(&mut self, _owner: usize, _settings: bool) {}
    pub(super) fn close_mod_screen(&mut self, _tell: bool) {}
    pub(super) fn draw_mod_screen(&mut self, _batch: &mut HudBatch) {}
    pub(super) fn mod_screen_click(&mut self, _button: winit::event::MouseButton) {}
    pub(super) fn mod_screen_key(&mut self, _key: winit::keyboard::KeyCode) -> bool {
        false
    }
    pub(super) fn mod_screen_scroll(&mut self, _delta: f32) {}
    pub(super) fn mod_ropes(&self) -> Vec<([f64; 3], u32, f32, f32, Option<[f64; 3]>)> {
        Vec::new()
    }
}

#[cfg(feature = "mods")]
impl Game<'_> {
    /// Reads `horizon-mods/` and the dev mods, and starts the ones turned on.
    pub(super) fn start_mods(&mut self) {
        let dir = Self::options_path().with_file_name(mod_list::DIR);
        let dev = &self.mods.dev;
        let mut paths = horizon_mods::find(&dir);
        paths.retain(|p| !dev.contains(p));
        paths.extend(dev.iter().cloned());
        // A dev mod is on whatever the list says.
        let dev_ids: Vec<String> = dev
            .iter()
            .filter_map(|p| horizon_mods::Package::read(p).ok())
            .map(|p| p.manifest.id)
            .collect();
        let enabled = &self.settings.mods.enabled;
        let on = |id: &str| enabled.contains(id) || dev_ids.iter().any(|d| d == id);
        let mods = Mods::load_for(self.mod_runs(), &paths, on, vanilla_command);
        self.mods.mods = mods;
        let chosen = &self.settings.mods;
        self.mods.mods.set_bindings(|id| chosen.keys_of(id));
        // Entries were replaced wholesale: a screen's owner index is stale.
        self.mods.screen = None;
        // Content mods may have changed the items: remade on the next read.
        self.mods.recipes = None;
        self.list_mods();
        self.sync_mod_commands();
        self.show_mod_output();
    }
    /// Which mods this game runs: a server the server mods, the window
    /// client mods, and both when it simulates the world itself (in-frame).
    fn mod_runs(&self) -> Runs {
        match self.options.execution_role {
            ExecutionRole::IntegratedServer | ExecutionRole::DedicatedServer => Runs::Server,
            ExecutionRole::Client if self.options.server_thread => Runs::Client,
            ExecutionRole::Client => Runs::Both,
        }
    }
    /// The Mods page's rows, from what loaded.
    fn list_mods(&mut self) {
        self.mods.server = self.mods.mods.runs_server();
        self.mods.world = self.mods.mods.wants_world();
        let dev = &self.mods.dev;
        self.settings.mods.installed = self
            .mods
            .mods
            .entries()
            .iter()
            .map(|e| {
                let file = e.path.file_name().unwrap_or_default().to_string_lossy();
                let problem = match &e.status {
                    Status::Failed(reason) => Some(format!("Stopped: {reason}")),
                    _ => None,
                };
                let Some(m) = &e.manifest else {
                    return Installed {
                        id: String::new(),
                        title: file.into_owned(),
                        description: String::new(),
                        problem,
                        dev: false,
                        has_settings: false,
                    };
                };
                // What it may do first: the row cuts long descriptions.
                let may: Vec<&str> = m.capabilities.iter().map(|c| c.describe()).collect();
                let may = if may.is_empty() {
                    "Asks for nothing.".to_owned()
                } else {
                    format!("May {}.", may.join(", "))
                };
                Installed {
                    id: m.id.clone(),
                    title: format!("{} {}", m.name, m.version),
                    description: format!("{may} {}", m.description).trim().to_owned(),
                    problem,
                    dev: dev.contains(&e.path),
                    has_settings: e.has_ui_settings(),
                }
            })
            .collect();
    }
    /// The chat's tree after the running mods changed.
    fn sync_mod_commands(&mut self) {
        // In-frame, the window runs server mods too: their names, free text
        // here (the server parses their arguments).
        let server = self.mods.mods.server_commands().map(|c| c.name.as_str());
        let mut names = self.mods.mods.commands().chain(server).peekable();
        self.mods.tree = names
            .peek()
            .is_some()
            .then(|| vanilla::game_dispatcher().with_mod_commands(names));
        self.merge_remote_tree();
    }

    /// The server's command tree with the mods' commands, for the chat.
    pub(super) fn merge_remote_tree(&mut self) {
        let names: Vec<String> = self.mods.mods.commands().map(str::to_owned).collect();
        self.mods.remote_tree = self
            .remote
            .as_ref()
            .and_then(|r| r.commands.as_ref())
            .filter(|_| !names.is_empty())
            .map(|t| t.with_mod_commands(names.iter().map(String::as_str)));
    }

    /// The running mods' keys for Controls, each mod's in the order it
    /// registered them.
    pub(super) fn mod_key_rows(&self) -> Vec<horizon_client::options_ui::ModKeyRow> {
        let mods = &self.mods.mods;
        mods.entries()
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, e.manifest.as_ref()?)))
            .flat_map(|(i, m)| {
                mods.keys_of(i)
                    .iter()
                    .map(|k| horizon_client::options_ui::ModKeyRow {
                        id: m.id.clone(),
                        title: m.name.clone(),
                        name: k.name.clone(),
                        label: k.label.clone(),
                        key: k.key.clone(),
                        default: k.default.clone(),
                    })
            })
            .collect()
    }
    /// The keys chosen in Controls (or on a mod's page) to the running mods.
    pub(super) fn apply_mod_keys(&mut self) {
        let chosen = &self.settings.mods;
        self.mods.mods.set_bindings(|id| chosen.keys_of(id));
    }
    /// After the options changed: restart the mods if the list of the ones
    /// turned on did.
    pub(super) fn sync_mods(&mut self) {
        let running: Vec<&str> = self
            .mods
            .mods
            .entries()
            .iter()
            .filter(|e| e.status != Status::Off)
            .filter_map(|e| Some(e.manifest.as_ref()?.id.as_str()))
            .collect();
        let wanted: Vec<&str> = self
            .settings
            .mods
            .installed
            .iter()
            .enumerate()
            .filter(|&(i, _)| self.settings.mods.is_on(i))
            .map(|(_, m)| m.id.as_str())
            .collect();
        if running != wanted {
            self.start_mods();
        }
    }
    /// What mods read this tick (or frame, for mods with a `frame` hook): the
    /// server's players as they were at the last tick's end.
    fn mod_snapshot(&mut self) -> Snapshot {
        let players = if self.mods.server {
            self.mods.players.clone()
        } else {
            Vec::new()
        };
        self.mod_snapshot_with(players)
    }
    /// What mods read, with these players on the server.
    pub(super) fn mod_snapshot_with(&mut self, players: Vec<ServerPlayer>) -> Snapshot {
        let player = self.player.as_ref().map(|p| Player {
            x: p.position.x,
            y: p.position.y,
            z: p.position.z,
            yaw: self.yaw,
            pitch: self.pitch,
            health: p.health.health(),
            food: p.health.food.food.max(0) as u32,
            dimension: self.dim.id().to_owned(),
        });
        let clock = player.is_some().then(|| Clock {
            game_time: self.world.game_time.max(0) as u64,
            day_time: self.world.day_time.max(0) as u64,
        });
        let (cps, mouse) = self.mouse_reading();
        // The movement and click mappings, as the keystroke overlay reads them:
        // keys by the default key their binding is translated to.
        let mappings = MAPPINGS
            .iter()
            .map(|&(name, key)| {
                let label = keybinds::display_name(self.settings.keys.get(name)).plain();
                let down = match key {
                    Some(k) => self.keys.contains(&k),
                    None => mouse[usize::from(name == "key.use")],
                };
                (name, label, down)
            })
            .collect();
        if self.mods.font_widths.is_empty() {
            let atlas = &self.gui.atlas;
            self.mods.font_widths = (0..FONT_CHARS)
                .map(|c| char::from_u32(c).map_or(6.0, |c| atlas.glyph_cell(c).1))
                .collect();
        }
        let (recipes, known_recipes) = self.mod_recipes();
        let motion = self
            .mods
            .mods
            .wants_motion()
            .then(|| self.motion_state())
            .filter(|_| self.motion_allowed())
            .flatten();
        Snapshot {
            player,
            clock,
            gui_size: self.gui_size(),
            held: self.mod_held(),
            frame: self.frame_reading(),
            mappings,
            cps,
            font_widths: self.mods.font_widths.clone(),
            players,
            view_hit: self.mod_view_hit(),
            recipes,
            known_recipes,
            placing: self.mod_recipe_screen(),
            creative: self.creative,
            motion,
        }
    }
    /// The first block along the player's view within
    /// [`horizon_mods::RAYCAST_MAX`] and its distance from the eye
    /// (`game.raycast`): through the loaded chunks as the crosshair's ray
    /// goes (fluids pass), then across the distant terrain's ground heights,
    /// a block at a time.
    fn mod_view_hit(&self) -> Option<(f32, horizon_mods::Hit)> {
        if !self.mods.mods.wants_raycast() {
            return None;
        }
        let eye = self.eye_position()?;
        let dir = super::view_vector(self.yaw, self.pitch);
        let reach = f64::from(horizon_mods::RAYCAST_MAX);
        let (pos, point, state) = match self.clip_ray(eye, dir, reach) {
            Some(hit) => (
                hit.pos,
                hit.point,
                self.world.block(hit.pos[0], hit.pos[1], hit.pos[2]),
            ),
            None => self.lod_ray(eye, dir, reach)?,
        };
        let distance = (0..3).map(|i| (point[i] - eye[i]).powi(2)).sum::<f64>();
        let hit = horizon_mods::Hit {
            block_x: pos[0],
            block_y: pos[1],
            block_z: pos[2],
            x: point[0],
            y: point[1],
            z: point[2],
            block: self.res.facts().get(state).name.to_string(),
        };
        Some((distance.sqrt() as f32, hit))
    }
    /// Past the resident chunks: the view walked across the distant
    /// terrain's ground heights (`Lod::ground`), a block at a time. Columns in
    /// loaded chunks are left to `clip_ray`, which has already missed them.
    fn lod_ray(
        &self,
        eye: [f64; 3],
        dir: [f64; 3],
        reach: f64,
    ) -> Option<([i32; 3], [f64; 3], horizon_world::registry::BlockStateId)> {
        let lod = self.lod.as_ref()?;
        let mut t = 0.0;
        while t <= reach {
            let p = [0, 1, 2].map(|i| eye[i] + dir[i] * t);
            let (x, z) = (p[0].floor() as i32, p[2].floor() as i32);
            if !self.world.loaded(x, z)
                && let Some((height, state)) = lod.ground(x, z)
                && p[1] <= f64::from(height)
            {
                return Some(([x, height - 1, z], [p[0], f64::from(height), p[2]], state));
            }
            t += 1.0;
        }
        None
    }
    /// A mod's `command.run-quiet` lines (vanilla's commands only, as
    /// `command.run`): to a server that runs them quietly as one payload
    /// (`horizon_protocol::quiet`), to any other as chat commands, or for this
    /// window's own world next tick.
    fn mod_quiet_commands(&mut self, mod_id: String, lines: Vec<String>) {
        let lines: Vec<String> = lines
            .into_iter()
            .filter(|l| vanilla_command(l.split(' ').next().unwrap_or_default()))
            .collect();
        match self.remote.as_ref().map(|r| r.quiet_commands) {
            Some(true) => {
                for data in horizon_protocol::quiet::encode(&mod_id, &lines) {
                    self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                        horizon_protocol::packets::configuration::CustomPayload {
                            channel: horizon_protocol::quiet::CHANNEL.into(),
                            data,
                        },
                    ));
                }
            }
            // The server checks the player's permissions; its feedback shows.
            Some(false) => {
                for line in lines {
                    self.send_chat(&format!("/{line}"));
                }
            }
            None => self.mods.quiet.push((mod_id, lines)),
        }
    }
    /// A quiet batch in this window's own world, as a server runs one
    /// (game/lan_quiet.rs): each line as the player typing it, no chat, a
    /// chat message refused; one line in the log.
    fn run_quiet_batch(&mut self, mod_id: &str, lines: &[String]) {
        let permission = self.ui.registries.permission;
        let mut failed = 0;
        for line in lines {
            let ok = self.signable_arguments(permission, line).is_empty()
                && self.run_command_quietly(line);
            failed += usize::from(!ok);
        }
        log::info!(
            "[mod {mod_id}] ran {} quiet command(s): {failed} failed",
            lines.len()
        );
    }
    /// Capability `motion`'s view: velocity, the eyes and the view; each
    /// mod's ray hits are filled in by [`Game::cast_motion_rays`].
    fn motion_state(&self) -> Option<MotionState> {
        let p = self.player.as_ref()?;
        let eye = self.eye_position()?;
        let vec = |v: [f64; 3]| MotionVec {
            x: v[0],
            y: v[1],
            z: v[2],
        };
        // The view follows the wall camera's turn: a ray along it stays
        // under the crosshair.
        let look = self.mod_wall_turn()
            * glam::DVec3::from_array(super::view_vector(self.yaw, self.pitch));
        Some(MotionState {
            velocity: vec([p.velocity.x, p.velocity.y, p.velocity.z]),
            on_ground: p.ground.on_ground,
            eye: vec(eye),
            look: vec(look.to_array()),
            hits: Vec::new(),
        })
    }
    /// Capability `motion` acts only in the player's own world: single
    /// player, or a LAN world they host. On another server a motion mod
    /// reads nothing and moves nothing, and that server sees the player as
    /// it sees any other.
    pub(super) fn motion_allowed(&self) -> bool {
        self.remote
            .as_ref()
            .is_none_or(|r| r.session.address == "local")
    }
    /// Forgets what motion mods asked for: none runs, or not here.
    fn clear_motion(&mut self) {
        self.mods.safe_fall = false;
        self.mods.pose = [None, None];
        self.mods.wind = None;
        self.mods.camera = None;
        self.mods.cam_ease.set(None);
        self.mods.wall_ease.set((0.0, None));
    }
    /// Capability `motion`: casts each running mod's rays from where the
    /// player is now, before the mods tick; [`RAY_BUDGET`] a tick in all.
    fn cast_motion_rays(&mut self) {
        use horizon_core::geometry::Axis;
        /// Rays cast a tick across every mod; the rest find nothing.
        const RAY_BUDGET: usize = 256;
        if !self.motion_allowed() {
            return;
        }
        let (Some(p), Some(eye)) = (self.player.as_ref(), self.eye_position()) else {
            return;
        };
        let mut budget = RAY_BUDGET;
        let feet = [p.position.x, p.position.y, p.position.z];
        let world = self.mob_world_base(&[]);
        let solid = |pos: [i32; 3]| {
            let boxes: Vec<[f64; 6]> = world
                .block_collision_boxes(pos[0], pos[1], pos[2])
                .into_iter()
                .map(|b| {
                    [
                        b.min(Axis::X),
                        b.min(Axis::Y),
                        b.min(Axis::Z),
                        b.max(Axis::X),
                        b.max(Axis::Y),
                        b.max(Axis::Z),
                    ]
                })
                .collect();
            (!boxes.is_empty()).then_some(boxes)
        };
        let all: Vec<_> = self
            .mods
            .mods
            .motion_rays()
            .into_iter()
            .map(|(i, rays)| {
                let hits = rays
                    .iter()
                    .map(|r| {
                        if budget == 0 {
                            return None;
                        }
                        budget -= 1;
                        let base = if r.from_eyes { eye } else { feet };
                        let from = [
                            base[0] + r.offset.x,
                            base[1] + r.offset.y,
                            base[2] + r.offset.z,
                        ];
                        let dir = glam::DVec3::new(r.dir.x, r.dir.y, r.dir.z).normalize_or_zero();
                        if dir == glam::DVec3::ZERO {
                            return None;
                        }
                        let dir = dir.to_array();
                        let hit = if r.solid {
                            self.clip_boxes(from, dir, r.distance, solid)
                        } else {
                            self.clip_ray(from, dir, r.distance)
                        };
                        hit.map(|h| {
                            glam::DVec3::from_array(h.point).distance(glam::DVec3::from_array(from))
                        })
                    })
                    .collect();
                (i, hits)
            })
            .collect();
        for (i, hits) in all {
            self.mods.mods.set_motion_hits(i, hits);
        }
    }
    /// Every running mod's ropes: where each ends and its 0xRRGGBB.
    pub(super) fn mod_ropes(&self) -> Vec<([f64; 3], u32, f32, f32, Option<[f64; 3]>)> {
        if !self.motion_allowed() {
            return Vec::new();
        }
        self.mods
            .mods
            .ropes()
            .map(|r| {
                (
                    [r.to.x, r.to.y, r.to.z],
                    r.color,
                    r.progress,
                    r.wave,
                    r.from.as_ref().map(|f| [f.x, f.y, f.z]),
                )
            })
            .collect()
    }
    /// A game tick.
    pub(super) fn tick_mods(&mut self) {
        // Here, at the player's level: shown inside a console call (a control
        // socket command set the hook off), they would run as the console.
        for line in std::mem::take(&mut self.mods.commands) {
            self.run_command(&line);
        }
        for (mod_id, lines) in std::mem::take(&mut self.mods.quiet) {
            self.run_quiet_batch(&mod_id, &lines);
        }
        if !self.mods.mods.is_running() {
            self.clear_motion();
            return;
        }
        self.tick_mod_use();
        let snapshot = self.mod_snapshot();
        self.mods.mods.set_snapshot(snapshot);
        self.cast_motion_rays();
        self.mods.mods.tick(self.ticks);
        // Capability `motion`: what the mods asked of the player, for its
        // next move. Only in the player's own world.
        let request = self.mods.mods.take_motion();
        if !self.motion_allowed() {
            self.clear_motion();
            self.show_mod_output();
            return;
        }
        self.mods.safe_fall = request.reset_fall;
        let pose = self
            .mods
            .mods
            .pose()
            .map(
                |(parts, [pitch, roll, twist], facing, shift)| horizon_client::mobs::ModPose {
                    parts: parts
                        .iter()
                        .map(|p| horizon_client::mobs::ModPartPose {
                            part: p.part.clone(),
                            rotation: [p.pitch, p.yaw, p.roll],
                            pivot_y: p.pivot_y,
                            pivot_z: p.pivot_z,
                        })
                        .collect(),
                    pitch: *pitch,
                    roll: *roll,
                    twist: *twist,
                    facing: *facing,
                    shift: *shift,
                },
            );
        self.mods.pose = [self.mods.pose[1].take(), pose];
        self.mods.wind = self.mods.mods.wind();
        self.mods.camera = self.mods.mods.camera();
        if let Some(p) = self.player.as_mut() {
            if let Some(v) = request.velocity {
                p.velocity = horizon_core::geometry::Vec3::new(v[0], v[1], v[2]);
            }
            if request.reset_fall {
                p.fall_distance = 0.0;
            }
        }
        self.show_mod_output();
    }
    /// Restarts the dev mods rebuilt since the last check, once a second.
    pub(super) fn reload_dev_mods(&mut self) {
        if self.mods.dev.is_empty()
            || self
                .mods
                .checked
                .is_some_and(|t| t.elapsed() < RELOAD_CHECK)
        {
            return;
        }
        self.mods.checked = Some(Instant::now());
        if self.mods.mods.reload_changed(vanilla_command) {
            let chosen = &self.settings.mods;
            self.mods.mods.set_bindings(|id| chosen.keys_of(id));
            self.list_mods();
            self.sync_mod_commands();
            if self.options.execution_role.is_client() {
                self.system_message(Text::literal("Mods reloaded").with_color(text::GRAY));
            } else {
                log::info!("Horizon: mods reloaded");
            }
        }
    }
    /// Once per frame: the frame hook, and dev mods' rebuilds.
    pub(super) fn mods_frame(&mut self, elapsed: Duration) {
        self.reload_dev_mods();
        if !self.mods.mods.is_running() {
            return;
        }
        self.mod_input_frame();
        if self.mods.mods.wants_frames() {
            let snapshot = self.mod_snapshot();
            self.mods.mods.set_snapshot(snapshot);
        }
        self.mods.mods.frame(elapsed.as_secs_f32());
        self.show_mod_output();
    }
    /// `/command` typed in chat: true when a mod owns it.
    pub(super) fn mod_command(&mut self, command: &str) -> bool {
        if !self.mods.mods.is_running() {
            return false;
        }
        // The command may follow a teleport in the same tick.
        let snapshot = self.mod_snapshot();
        self.mods.mods.set_snapshot(snapshot);
        if !self.mods.mods.command(command) {
            return false;
        }
        self.show_mod_output();
        true
    }
    /// A chat line the player received.
    pub(super) fn mods_chat(&mut self, line: &str) {
        self.mods.mods.chat(line);
        self.show_mod_output();
    }
    /// What mods showed: client mods' lines into the chat, server mods'
    /// to their players.
    pub(super) fn show_mod_output(&mut self) {
        self.route_mod_output(None);
    }
    /// [`Game::show_mod_output`], with the hub when it is out of `self.lan`.
    pub(super) fn route_mod_output(&mut self, mut host: Option<&mut LanHost>) {
        for output in self.mods.mods.drain() {
            let line = match output {
                Output::Chat { name, text } => {
                    let mut line = Text::literal(format!("[{name}] ")).with_color(text::GRAY);
                    line.push(Text::literal(text));
                    line
                }
                Output::Feedback { text, error: false } => Text::literal(text),
                Output::Feedback { text, error: true } => Text::literal(text).with_color(text::RED),
                // Vanilla's commands only: a mod running a mod command could loop.
                Output::Command { line } => {
                    let name = line.split(' ').next().unwrap_or_default();
                    if !vanilla_command(name) {
                        // Dropped.
                    } else if self.remote.is_some() {
                        // The server checks the player's permissions.
                        self.send_chat(&format!("/{line}"));
                    } else {
                        self.mods.commands.push(line);
                    }
                    continue;
                }
                Output::QuietCommands { mod_id, lines } => {
                    self.mod_quiet_commands(mod_id, lines);
                    continue;
                }
                Output::Stopped { name, reason } => {
                    // Its screen goes with it: the entry's run is gone, so
                    // `mod_ui_view` finds no widgets and the screen closes.
                    self.list_mods();
                    self.sync_mod_commands();
                    let line = format!("Mod {name} stopped: {reason}");
                    if self.options.dedicated.is_some() {
                        super::dedicated::log("WARN", &line);
                    }
                    Text::literal(line).with_color(text::RED)
                }
                server @ (Output::Tell { .. }
                | Output::Broadcast { .. }
                | Output::Reply { .. }
                | Output::SetBlocks { .. }) => {
                    self.route_server_output(host.as_deref_mut(), server);
                    continue;
                }
                Output::OpenScreen { owner } => {
                    self.open_mod_screen(owner, false);
                    continue;
                }
                Output::Shake { strength, seconds } => {
                    self.mods.shake.add(strength, seconds);
                    continue;
                }
                Output::PlaceRecipe { book_id, all } => {
                    self.mod_place_recipe(book_id, all);
                    continue;
                }
                Output::Give {
                    item,
                    count,
                    components,
                } => {
                    self.mod_give(&item, count, &components);
                    continue;
                }
                Output::CloseScreen { owner } => {
                    if self.mods.screen.as_ref().is_some_and(|s| s.owner == owner) {
                        self.close_mod_screen(false);
                    }
                    continue;
                }
            };
            self.add_mod_chat_line(line);
        }
        // A mod may set or clear its settings page in any call, not only
        // while loading: its row's Settings button follows.
        let rows = self.settings.mods.installed.iter_mut();
        for (row, e) in rows.zip(self.mods.mods.entries()) {
            row.has_settings = e.has_ui_settings();
        }
    }
    /// A mod's line in the window's chat; not through system_message: a mod
    /// must not hear its own lines.
    /// Written by this client, so the Chat option doesn't hide it.
    pub(super) fn add_mod_chat_line(&mut self, line: Text) {
        let focused = self.screen == Screen::Chat;
        self.ui.chat.add(
            &self.gui.atlas,
            line,
            None,
            horizon_client::chat::ChatSource::Client,
            self.ticks,
            focused,
        );
    }
    /// The mods' HUD panels, stacked with the extras in their corners.
    pub(super) fn draw_mod_panels(&self, batch: &mut HudBatch, corners: &mut Corners) {
        for panel in self.mods.mods.panels() {
            let corner = match panel.corner {
                horizon_mods::Corner::TopLeft => Corner::TopLeft,
                horizon_mods::Corner::TopRight => Corner::TopRight,
                horizon_mods::Corner::BottomLeft => Corner::BottomLeft,
                horizon_mods::Corner::BottomRight => Corner::BottomRight,
            };
            // F3 takes the top left, as it does from the extras.
            if self.debug && corner == Corner::TopLeft {
                continue;
            }
            let lines: Vec<(&str, u32)> = panel
                .lines
                .iter()
                .map(|l| (l.text.as_str(), l.color))
                .collect();
            extras_hud::draw_lines(batch, &self.gui.atlas, corners, corner, &lines);
        }
        // Capability `render`'s corner panels, placed the same way.
        for panel in self.mods.mods.corner_panels() {
            let corner = match panel.corner {
                horizon_mods::RenderCorner::TopLeft => Corner::TopLeft,
                horizon_mods::RenderCorner::TopRight => Corner::TopRight,
                horizon_mods::RenderCorner::BottomLeft => Corner::BottomLeft,
                horizon_mods::RenderCorner::BottomRight => Corner::BottomRight,
            };
            if self.debug && corner == Corner::TopLeft {
                continue;
            }
            let at = corners.place(corner, panel.width, panel.height);
            self.draw_shapes(batch, &panel.shapes, at);
        }
    }

    /// A `ui` screen's title and widgets as the mod last pushed them; None
    /// when the surface is gone (the mod stopped or closed it). Borrows
    /// `mods` only, so `self.mods.screen` may stay borrowed for the scrolls.
    fn mod_ui_view<'a>(mods: &'a Mods, ms: &ModScreen) -> Option<(&'a str, &'a [UiWidget])> {
        if ms.settings {
            let widgets = mods.settings_widgets(ms.owner);
            if widgets.is_empty() && mods.keys_of(ms.owner).is_empty() {
                return None;
            }
            let name = mods
                .entries()
                .get(ms.owner)
                .and_then(|e| e.manifest.as_ref())
                .map_or("Mod", |m| m.name.as_str());
            Some((name, widgets))
        } else {
            let s: &UiScreen = mods.screen(ms.owner)?;
            Some((s.title.as_str(), s.widgets.as_slice()))
        }
    }
    /// The keys a settings page lists first: the mod's own (`input`).
    fn mod_page_keys<'a>(mods: &'a Mods, ms: &ModScreen) -> &'a [ModKey] {
        if ms.settings {
            mods.keys_of(ms.owner)
        } else {
            &[]
        }
    }
    /// A key row's label as Key Binds shows one, "Zoom: Z", marked
    /// "> Zoom: Z <" in yellow while it waits for the new key.
    fn mod_key_label(k: &ModKey, waiting: bool) -> Text {
        let value = format!("{}: {}", k.label, keybinds::display_name(&k.key).plain());
        if waiting {
            Text::literal(format!("> {value} <")).with_color(text::YELLOW)
        } else {
            Text::literal(value)
        }
    }
    /// The player bound the key row that waited to `key`: the mod's key
    /// follows it now and at the next start (`horizon-mods.txt`).
    fn bind_mod_key(&mut self, key: String) {
        let Some(ms) = self.mods.screen.as_mut() else {
            return;
        };
        let Some(name) = ms.binding.take() else {
            return;
        };
        let owner = ms.owner;
        if !self.mods.mods.rebind(owner, &name, &key) {
            return;
        }
        let Some(id) = self
            .mods
            .mods
            .entries()
            .get(owner)
            .and_then(|e| e.manifest.as_ref())
            .map(|m| m.id.clone())
        else {
            return;
        };
        self.settings
            .mods
            .keys
            .entry(id)
            .or_default()
            .insert(name, key);
        self.settings.save(&Self::options_path());
    }
    /// A key while a mod screen is up: true when a key row waited for
    /// it (Key Binds: Escape binds none).
    pub(super) fn mod_screen_key(&mut self, key: KeyCode) -> bool {
        if self
            .mods
            .screen
            .as_ref()
            .is_none_or(|ms| ms.binding.is_none())
        {
            return false;
        }
        let value = match key {
            KeyCode::Escape => None,
            k => keybinds::key_name(k),
        };
        self.bind_mod_key(value.unwrap_or("key.keyboard.unknown").to_owned());
        true
    }
    /// The layout the mod screen's widgets get at `h`: (widget, top,
    /// height), the column being `COLUMN` wide down the middle.
    fn mod_ui_layout(h: f32, top: f32, widgets: &[UiWidget]) -> Vec<(f32, f32)> {
        let mut out = Vec::with_capacity(widgets.len());
        let mut y = top;
        for w in widgets {
            let wh = match w {
                UiWidget::Text(_) => 12.0,
                UiWidget::Item(_) | UiWidget::Button(_) => 20.0,
                UiWidget::List(l) => (l.rows.len().min(6) as f32 * 12.0) + 8.0,
            };
            // Nothing may cover Done; a crowded mod loses what won't fit.
            if y + wh > h - 34.0 {
                break;
            }
            out.push((y, wh));
            y += wh + 6.0;
        }
        out
    }
    /// A mod pushes its screen (`ui.open-screen`), or Options > Mods opens
    /// its settings page (`settings`, the mod's `installed` index).
    pub(super) fn open_mod_screen(&mut self, owner: usize, settings: bool) {
        if self.title.is_some() {
            return;
        }
        if settings {
            self.mods.mods.ui_settings_opened(owner);
            let keys = self.mods.mods.keys_of(owner);
            if self.mods.mods.settings_widgets(owner).is_empty() && keys.is_empty() {
                self.show_mod_output();
                return;
            }
        }
        self.mods.screen = Some(ModScreen {
            owner,
            settings,
            under: self.screen,
            scrolls: Vec::new(),
            binding: None,
        });
        self.screen = Screen::Mod;
        if self.player.is_some() {
            self.capture(false);
        }
        self.show_mod_output();
    }
    /// Leaves the mod screen: back to Options for a settings page, to what
    /// it covered for a mod's own. `tell` is Esc/Done, heard as `closed`.
    pub(super) fn close_mod_screen(&mut self, tell: bool) {
        let Some(ms) = self.mods.screen.take() else {
            return;
        };
        if ms.settings {
            self.open_options_at(self.options_from_title, Page::Mods);
        } else {
            self.screen = ms.under;
            if self.screen == Screen::Playing && self.player.is_some() {
                self.capture(true);
            }
        }
        if tell {
            self.mods.mods.ui_closed(ms.owner);
            self.show_mod_output();
        }
    }
    /// The mod's screen or settings page: its title over widgets stacked
    /// down the middle, a Done button, Esc to leave (vanilla's menus).
    pub(super) fn draw_mod_screen(&mut self, batch: &mut HudBatch) {
        let Some(ms) = self.mods.screen.as_mut() else {
            // The screen was dropped while it was up: back to the world,
            // mouse held again like any return to Playing.
            self.screen = Screen::Playing;
            if self.player.is_some() {
                self.capture(true);
            }
            return;
        };
        let Some((title, widgets)) = Self::mod_ui_view(&self.mods.mods, ms) else {
            // The mod's surface is gone (it stopped or cleared it): leave
            // the screen like Done did, without `closed` — the mod ended it.
            self.close_mod_screen(false);
            return;
        };
        // Item icons draw in a second pass: `draw_item` needs `&mut self`,
        // so `atlas` may not stay borrowed across it.
        let mut items: Vec<(String, f32, f32)> = Vec::new();
        let atlas = &self.gui.atlas;
        let in_world = self.player.is_some();
        ui::menu_background(batch, atlas, 0.0, 0.0, batch.width, batch.height, in_world);
        ui::centered_text(
            batch,
            atlas,
            &Text::literal(title),
            0.0,
            15.0,
            batch.width,
            u32::MAX,
        );
        let cx = (batch.width / 2.0).floor();
        let x = cx - MOD_UI_COLUMN / 2.0;
        // A settings page starts with the mod's keys, as Key Binds rows.
        let keys = Self::mod_page_keys(&self.mods.mods, ms);
        for (i, k) in keys.iter().enumerate() {
            let wy = MOD_UI_TOP + i as f32 * MOD_UI_KEY_ROW;
            let hover = ui::hovered(self.cursor, x, wy, MOD_UI_COLUMN, 20.0);
            let waiting = ms.binding.as_deref() == Some(k.name.as_str());
            let label = Self::mod_key_label(k, waiting);
            ui::button(
                batch,
                atlas,
                x,
                wy,
                MOD_UI_COLUMN,
                20.0,
                &label,
                true,
                hover,
            );
        }
        let top = MOD_UI_TOP + keys.len() as f32 * MOD_UI_KEY_ROW;
        let layout = Self::mod_ui_layout(batch.height, top, widgets);
        let mut list_i = 0usize;
        for (w, &(wy, wh)) in widgets.iter().zip(&layout) {
            match w {
                UiWidget::Button(b) => {
                    let hover = ui::hovered(self.cursor, x, wy, MOD_UI_COLUMN, 20.0);
                    let label = Text::literal(b.label.clone());
                    ui::button(
                        batch,
                        atlas,
                        x,
                        wy,
                        MOD_UI_COLUMN,
                        20.0,
                        &label,
                        true,
                        hover,
                    );
                }
                UiWidget::Text(t) => {
                    let color = if t.color == 0 {
                        u32::MAX
                    } else {
                        t.color | 0xFF00_0000
                    };
                    ui::centered_text(
                        batch,
                        atlas,
                        &Text::literal(t.text.clone()),
                        x,
                        wy + 1.0,
                        MOD_UI_COLUMN,
                        color,
                    );
                }
                UiWidget::Item(i) => {
                    if self.res.items().get(&i.item).is_some() {
                        items.push((i.item.clone(), x + 2.0, wy + 2.0));
                    } else {
                        batch.text(atlas, "?", x + 6.0, wy + 6.0, u32::MAX, true);
                    }
                    let name = if i.name.is_empty() {
                        i.item.rsplit(':').next().unwrap_or(&i.item).to_string()
                    } else {
                        i.name.clone()
                    };
                    batch.text_styled(
                        atlas,
                        &Text::literal(name),
                        x + 22.0,
                        wy + 6.0,
                        u32::MAX,
                        true,
                    );
                }
                UiWidget::List(l) => {
                    ms.scrolls.resize_with(list_i + 1, ui::Scroll::default);
                    let scroll = ms.scrolls[list_i];
                    list_i += 1;
                    let rows_top = wy + 4.0;
                    let rows_bottom = wy + wh - 4.0;
                    batch.fill(atlas, x, wy, MOD_UI_COLUMN, wh, 0x8000_0000);
                    for (b, yy) in [(x, wy), (x, wy + wh - 1.0)] {
                        batch.fill(atlas, b, yy, MOD_UI_COLUMN, 1.0, 0xFF50_5050);
                    }
                    for (xx, yy) in [(x, wy), (x + MOD_UI_COLUMN - 1.0, wy)] {
                        batch.fill(atlas, xx, yy, 1.0, wh, 0xFF50_5050);
                    }
                    let content = l.rows.len() as f32 * 12.0;
                    let first = (scroll.amount / 12.0) as usize;
                    let last = (first + ((rows_bottom - rows_top) / 12.0).ceil() as usize + 1)
                        .min(l.rows.len());
                    for (vi, ri) in (first..last).enumerate() {
                        let ry = rows_top + vi as f32 * 12.0 - (scroll.amount % 12.0);
                        if ui::hovered(self.cursor, x, ry, MOD_UI_COLUMN - 6.0, 12.0) {
                            batch.fill(atlas, x + 1.0, ry, MOD_UI_COLUMN - 8.0, 12.0, 0x20FF_FFFF);
                        }
                        if l.selected == Some(ri as u32) {
                            batch.fill(atlas, x + 1.0, ry, 3.0, 12.0, 0xFFFF_FFFF);
                        }
                        batch.text_styled(
                            atlas,
                            &Text::literal(l.rows[ri].clone()),
                            x + 6.0,
                            ry + 2.0,
                            u32::MAX,
                            true,
                        );
                    }
                    ui::scrollbar(
                        batch,
                        atlas,
                        x + MOD_UI_COLUMN - 5.0,
                        rows_top,
                        rows_bottom,
                        content,
                        scroll.amount,
                    );
                }
            }
        }
        let hover = ui::hovered(self.cursor, cx - 100.0, batch.height - 27.0, 200.0, 20.0);
        ui::button(
            batch,
            atlas,
            cx - 100.0,
            batch.height - 27.0,
            200.0,
            20.0,
            &Text::literal("Done"),
            true,
            hover,
        );
        for (id, ix, iy) in items {
            let stack = ItemStack::new(id.as_str(), 1);
            self.draw_item(batch, &stack, ix, iy);
        }
    }
    /// A click on the mod screen: Done leaves it, a `list` row is the mod's
    /// `ui-events::select`.
    pub(super) fn mod_screen_click(&mut self, button: MouseButton) {
        // Key Binds: a mouse button pressed while waiting binds it.
        if self
            .mods
            .screen
            .as_ref()
            .is_some_and(|ms| ms.binding.is_some())
        {
            self.bind_mod_key(keybinds::mouse_name(button));
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        let (w, h) = self.gui_size();
        let cursor = self.cursor;
        if ui::hovered(cursor, (w / 2.0).floor() - 100.0, h - 27.0, 200.0, 20.0) {
            self.audio.play_button_click();
            self.close_mod_screen(true);
            return;
        }
        let Some(ms) = self.mods.screen.as_ref() else {
            return;
        };
        let Some((_, widgets)) = Self::mod_ui_view(&self.mods.mods, ms) else {
            return;
        };
        let owner = ms.owner;
        let x = (w / 2.0).floor() - MOD_UI_COLUMN / 2.0;
        // A key row waits for the key to bind.
        let keys = Self::mod_page_keys(&self.mods.mods, ms);
        let picked = (0..keys.len()).find(|&i| {
            let wy = MOD_UI_TOP + i as f32 * MOD_UI_KEY_ROW;
            ui::hovered(cursor, x, wy, MOD_UI_COLUMN, 20.0)
        });
        if let Some(i) = picked {
            let name = keys[i].name.clone();
            self.audio.play_button_click();
            if let Some(ms) = self.mods.screen.as_mut() {
                ms.binding = Some(name);
            }
            return;
        }
        let top = MOD_UI_TOP + keys.len() as f32 * MOD_UI_KEY_ROW;
        let layout = Self::mod_ui_layout(h, top, widgets);
        let mut list_i = 0usize;
        // The `ui_select` call needs `&mut`: find the pick first.
        let mut hit = None;
        for (w, &(wy, wh)) in widgets.iter().zip(&layout) {
            // A button is `select(id, 0)`.
            if let UiWidget::Button(b) = w
                && ui::hovered(cursor, x, wy, MOD_UI_COLUMN, 20.0)
            {
                self.audio.play_button_click();
                let owner = self.mods.screen.as_ref().map_or(0, |ms| ms.owner);
                self.mods.mods.ui_select(owner, b.id, 0);
                self.show_mod_output();
                return;
            }
            let UiWidget::List(l) = w else { continue };
            let scroll = ms.scrolls.get(list_i).copied().unwrap_or_default();
            list_i += 1;
            let row = ((cursor.1 - (wy + 4.0) + scroll.amount) / 12.0) as i64;
            if ui::hovered(cursor, x + 1.0, wy + 1.0, MOD_UI_COLUMN - 7.0, wh - 2.0)
                && row >= 0
                && (row as usize) < l.rows.len()
            {
                hit = Some((l.id, row as u32));
                break;
            }
        }
        if let Some((id, row)) = hit {
            self.audio.play_button_click();
            self.mods.mods.ui_select(owner, id, row);
            self.show_mod_output();
        }
    }
    /// The wheel over a `list` scrolls it.
    pub(super) fn mod_screen_scroll(&mut self, delta: f32) {
        let (w, h) = self.gui_size();
        let cursor = self.cursor;
        let Some(ms) = self.mods.screen.as_mut() else {
            return;
        };
        let Some((_, widgets)) = Self::mod_ui_view(&self.mods.mods, ms) else {
            return;
        };
        let x = (w / 2.0).floor() - MOD_UI_COLUMN / 2.0;
        let top =
            MOD_UI_TOP + Self::mod_page_keys(&self.mods.mods, ms).len() as f32 * MOD_UI_KEY_ROW;
        let layout = Self::mod_ui_layout(h, top, widgets);
        let mut list_i = 0usize;
        for (w, &(wy, wh)) in widgets.iter().zip(&layout) {
            let UiWidget::List(l) = w else { continue };
            let over = ui::hovered(cursor, x, wy, MOD_UI_COLUMN, wh);
            ms.scrolls.resize_with(list_i + 1, ui::Scroll::default);
            if over {
                let content = l.rows.len() as f32 * 12.0;
                ms.scrolls[list_i].wheel(delta, 12.0, wy + 4.0, wy + wh - 4.0, content);
                return;
            }
            list_i += 1;
        }
    }
}

#[cfg(feature = "mods")]
const MOD_UI_TOP: f32 = 40.0;
#[cfg(feature = "mods")]
const MOD_UI_COLUMN: f32 = 200.0;
/// A key row's height with its gap, as the widgets below it space out.
#[cfg(feature = "mods")]
const MOD_UI_KEY_ROW: f32 = 26.0;
/// How far from the player a mod's fixed camera may stand, in blocks.
#[cfg(feature = "mods")]
const FIXED_CAMERA_REACH: f64 = 16.0;
impl Game<'_> {
    /// The mod's pose for the player's model this frame: the last two ticks'
    /// blended by `alpha`.
    pub(super) fn mod_pose(&self, alpha: f32) -> Option<horizon_client::mobs::ModPose> {
        let [prev, cur] = &self.mods.pose;
        let mut pose = cur.clone()?;
        let Some(prev) = prev else {
            return Some(pose);
        };
        let lerp = |a: f32, b: f32| {
            // A jump of more than half a turn (a flip ending) is not blended.
            if (b - a).abs() > std::f32::consts::PI {
                b
            } else {
                a + (b - a) * alpha
            }
        };
        pose.pitch = lerp(prev.pitch, pose.pitch);
        pose.roll = lerp(prev.roll, pose.roll);
        pose.twist = lerp(prev.twist, pose.twist);
        for part in &mut pose.parts {
            if let Some(p) = prev.parts.iter().find(|p| p.part == part.part) {
                for i in 0..3 {
                    part.rotation[i] = lerp(p.rotation[i], part.rotation[i]);
                }
                if let (Some(a), Some(b)) = (p.pivot_y, part.pivot_y) {
                    part.pivot_y = Some(lerp(a, b));
                }
                if let (Some(a), Some(b)) = (p.pivot_z, part.pivot_z) {
                    part.pivot_z = Some(lerp(a, b));
                }
            }
        }
        Some(pose)
    }
}

#[cfg(feature = "mods")]
impl Game<'_> {
    /// The wall camera's turn of the whole view (identity off the wall): the
    /// same turn applies to the aim, so the crosshair stays true.
    pub(super) fn mod_wall_turn(&self) -> glam::DQuat {
        match self.mods.camera.as_ref() {
            Some(horizon_mods::ModCamera::Chase(c)) if !self.camera_type.first_person() => {
                // Ease toward the mod's blend every frame, smoothly.
                let now = std::time::Instant::now();
                let (cur, then) = self.mods.wall_ease.get();
                let dt = then.map_or(0.0, |t| now.duration_since(t).as_secs_f64().min(0.1));
                let want = f64::from(c.wall_blend);
                let k = cur + (want - cur) * (1.0 - (-8.0_f64 * dt).exp());
                self.mods.wall_ease.set((k, Some(now)));
                match &c.wall {
                    Some(w) if k > 1e-3 => {
                        let w = glam::DVec3::new(w.x, w.y, w.z).normalize_or_zero();
                        glam::DQuat::IDENTITY
                            .slerp(glam::DQuat::from_rotation_arc(glam::DVec3::Y, -w), k)
                    }
                    _ => glam::DQuat::IDENTITY,
                }
            }
            _ => glam::DQuat::IDENTITY,
        }
    }
    /// True while a mod's fixed camera looks at the player.
    pub(super) fn mod_camera_fixed(&self) -> bool {
        matches!(self.mods.camera, Some(horizon_mods::ModCamera::Fixed(_)))
    }
    /// A mod's camera: the position, yaw and pitch to render
    /// from, or None for the game's own camera.
    pub(super) fn mod_camera_view(&self, partial: f32) -> Option<(glam::DVec3, f32, f32)> {
        horizon_client::camera::set_view_turn(None);
        let look_from = |from: glam::DVec3, to: glam::DVec3| {
            let d = (to - from).normalize_or_zero();
            let yaw = (-d.x).atan2(d.z).to_degrees() as f32;
            let pitch = (-d.y.clamp(-1.0, 1.0)).asin().to_degrees() as f32;
            (from, yaw, pitch)
        };
        match self.mods.camera.as_ref()? {
            horizon_mods::ModCamera::Fixed(f) => {
                // Near the player: a view of the body, not a free camera.
                let p = self.player.as_ref()?;
                let body = glam::DVec3::new(p.position.x, p.position.y, p.position.z);
                let near = |v: &horizon_mods::MotionVec| {
                    body + (glam::DVec3::new(v.x, v.y, v.z) - body)
                        .clamp_length_max(FIXED_CAMERA_REACH)
                };
                Some(look_from(near(&f.at), near(&f.look_at)))
            }
            horizon_mods::ModCamera::Chase(c) => {
                if self.camera_type.first_person() {
                    return None;
                }
                let p = self.player.as_ref()?;
                let a = f64::from(partial);
                let feet = glam::DVec3::new(
                    self.previous_position.x + (p.position.x - self.previous_position.x) * a,
                    self.previous_position.y + (p.position.y - self.previous_position.y) * a,
                    self.previous_position.z + (p.position.z - self.previous_position.z) * a,
                );
                let eye = feet + glam::DVec3::new(0.0, f64::from(p.dimensions.eye_height), 0.0);
                // The mouse turns the camera as always. On a tall wall the
                // whole view is turned (eased by `wall_blend`) so the wall is
                // the floor (Web of Shadows): the same mouse look, in a frame
                // whose up points away from the wall; looking at the wall
                // becomes looking up it.
                let turn = self.mod_wall_turn();
                let k = if turn.angle_between(glam::DQuat::IDENTITY) > 1e-4 {
                    1.0
                } else {
                    0.0
                };
                let up = turn * glam::DVec3::Y;
                let look = turn * glam::DVec3::from_array(super::view_vector(self.yaw, self.pitch));
                let right = look.cross(up).normalize_or_zero();
                let pivot = eye + up * f64::from(c.height);
                let reach = |side: f64| {
                    let want = pivot - look * f64::from(c.distance) + right * side;
                    let dir = want - pivot;
                    let len = dir.length();
                    let hit =
                        self.clip_ray(pivot.to_array(), (dir / len.max(1e-6)).to_array(), len);
                    match hit {
                        // Stop short of the wall the ray met.
                        Some(h) => {
                            let at = glam::DVec3::from_array(h.point);
                            let room = (at - pivot).length();
                            (
                                pivot + (at - pivot) * ((room - 0.4).max(0.0) / room.max(1e-6)),
                                room / len.max(1e-6),
                            )
                        }
                        None => (want, 1.0),
                    }
                };
                // A wall close behind: try the sides and take the roomier.
                let side = f64::from(c.side);
                let (mut at, room) = reach(side);
                let mut want_side = side;
                if room < 0.6 {
                    for s in [side + 3.0, side - 3.0] {
                        let (alt, r) = reach(s);
                        if r > room + 0.2 {
                            at = alt;
                            want_side = s;
                            break;
                        }
                    }
                }
                // Ease the distance and the side so the camera never jumps:
                // pulled in fast (no clipping), let out and slid slowly.
                let want_back = (pivot - at).dot(look).max(0.5);
                let now = std::time::Instant::now();
                let (back, side) = match self.mods.cam_ease.get() {
                    Some((back, side, then)) => {
                        let dt = now.duration_since(then).as_secs_f64().min(0.1);
                        let ease = |cur: f64, want: f64, rate: f64| {
                            cur + (want - cur) * (1.0 - (-rate * dt).exp())
                        };
                        let back = if want_back < back {
                            ease(back, want_back, 30.0)
                        } else {
                            ease(back, want_back, 3.0)
                        };
                        (back, ease(side, want_side, 4.0))
                    }
                    None => (want_back, want_side),
                };
                self.mods.cam_ease.set(Some((back, side, now)));
                let at = pivot - look * back + right * side;
                let (at, yaw, pitch) = look_from(at, at + look);
                // The roll that turns the view's own up onto `up`.
                // The whole orientation goes to the renderer: angles break
                // down when the view runs up a wall.
                if k > 0.0 {
                    let right = look.cross(up).normalize_or_zero();
                    let up = right.cross(look);
                    let basis =
                        glam::Mat3::from_cols(right.as_vec3(), up.as_vec3(), (-look).as_vec3());
                    horizon_client::camera::set_view_turn(Some(glam::Quat::from_mat3(&basis)));
                }
                Some((at, yaw, pitch))
            }
        }
    }
}
#[cfg(not(feature = "mods"))]
impl Game<'_> {
    pub(super) fn mod_camera_fixed(&self) -> bool {
        false
    }
    pub(super) fn mod_camera_view(&self, _partial: f32) -> Option<(glam::DVec3, f32, f32)> {
        None
    }
    pub(super) fn mod_wall_turn(&self) -> glam::DQuat {
        glam::DQuat::IDENTITY
    }
}
