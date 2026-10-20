use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{stdout, Stdout, Write};
use std::time::Duration;
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use serde::{Deserialize, Serialize};

use termion::color;
use termion::event::{Event, Key, MouseButton, MouseEvent};
use termion::input::{MouseTerminal, TermRead};
use termion::raw::{IntoRawMode, RawTerminal};
use termion::screen::{AlternateScreen, IntoAlternateScreen};

use futures::StreamExt;

use std::fmt::Write as _;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Receiver, Sender};

use tokio::sync::Notify;

use base64::Engine;
use image::imageops::{resize, FilterType};
use image::RgbImage;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use std::time::Instant;

use rspotify::prelude::*;
use rspotify::{AuthCodeSpotify, Credentials, OAuth, Config};
use rspotify::model::{AdditionalType, PlayableItem, PlaylistId, RepeatState, SearchType, SearchResult};
use rspotify::model::TrackId;

#[derive(Deserialize)]
struct ConfigFile {
  spotify: SpotifyConfig,
}

#[derive(Deserialize)]
struct SpotifyConfig {
  client_id: String,
  client_secret: String,
  redirect_uri: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
  Quit,
  PlayPause,
  Stop,
  Next,
  Previous,
  SeekForwards,
  SeekBackwards,
  ToggleShuffle,
  CycleLoopStatus,
  IncreaseVolume,
  DecreaseVolume,
  Search,
  PaneDown,
  PaneUp,
  PaneTop,
  PaneBottom,
  ToggleLike,
  AddToQueue,
  AddToPlaylist,
  PaneOpen,
  PaneTracks,
  RemoveFromPlaylist,
  PaneParent,
  PaneShowHidden,
  PaneHide,
  PanePin,
}

//uhh ion remember why i changed this variable,
//i was doing so much random stuff to get inputs working 😭
type Screen = MouseTerminal<AlternateScreen<RawTerminal<Stdout>>>;
const DEVICE_NAME: &str = "spotify-controller";
const FRAME_INTERVAL: u64 = 15;
const TICK_INTERVAL: u64 = 50;
const POLL_INTERVAL: u64 = 1500;
const DEVICE_WAIT_SECS: usize = 25;
const SPOTIFYD_START_SECS: usize = 45;
const MAX_PLAYLIST_TRACKS: usize = 500;
const SEARCH_PAGE: u32 = 10;
const MAX_RESULTS: usize = 300;

const ACTIONS: &[Action] = &[
  Action::Quit,
  Action::PlayPause,
  Action::Stop,
  Action::Next,
  Action::Previous,
  Action::SeekForwards,
  Action::SeekBackwards,
  Action::ToggleShuffle,
  Action::CycleLoopStatus,
  Action::IncreaseVolume,
  Action::DecreaseVolume,
  Action::Search,
  Action::PaneDown,
  Action::PaneUp,
  Action::PaneTop,
  Action::PaneBottom,
  Action::ToggleLike,
  Action::AddToQueue,
  Action::AddToPlaylist,
  Action::PaneOpen,
  Action::PaneTracks,
  Action::RemoveFromPlaylist,
  Action::PaneParent,
  Action::PaneShowHidden,
  Action::PaneHide,
  Action::PanePin,
];

fn sorted_actions() -> Vec<Action> {
  let mut actions = ACTIONS.to_vec();
  actions.sort_by_key(|action| action.label());
  actions
}

const BAR_ACTIONS: &[Action] = &[Action::Quit, Action::PlayPause, Action::Search];

impl Action {
  fn label(&self) -> &'static str {
    match *self {
      Action::Quit => "quit",
      Action::PlayPause => "play/pause",
      Action::Stop => "stop",
      Action::Next => "next",
      Action::Previous => "previous",
      Action::ToggleShuffle => "shuffle",
      Action::CycleLoopStatus => "repeat",
      Action::SeekForwards => "seek -> 5s",
      Action::SeekBackwards => "seek <- 5s",
      Action::IncreaseVolume => "vol +",
      Action::DecreaseVolume => "vol -",
      Action::Search => "search",
      Action::PaneDown => "scroll down",
      Action::PaneUp => "scroll",
      Action::PaneTop => "top of list",
      Action::PaneBottom => "bottom of list",
      Action::ToggleLike => "like/unlike",
      Action::AddToQueue => "add to queue",
      Action::AddToPlaylist => "add to playlist",
      Action::PaneOpen => "open/play",
      Action::PaneTracks => "view songs",
      Action::RemoveFromPlaylist => "remove from playlist",
      Action::PaneParent => "back",
      Action::PaneShowHidden => "show hidden",
      Action::PaneHide => "hide/unhide",
      Action::PanePin => "pin/unpin",
    }
  }

  fn is_pane(&self) -> bool {
    matches!(
      self,
      Action::PaneDown
        | Action::PaneUp
        | Action::PaneTop
        | Action::PaneBottom
        | Action::PaneOpen
        | Action::PaneTracks
        | Action::RemoveFromPlaylist
        | Action::PaneParent
        | Action::PaneShowHidden
        | Action::PaneHide
        | Action::PanePin
    )
  }

  fn default_keys(&self) -> Vec<Key> {
    match *self {
      Action::Quit => vec![Key::Char('q')],
      Action::PlayPause => vec![Key::Char(' ')],
      Action::Stop => vec![Key::Char('x')],
      Action::Next => vec![Key::Char('d')],
      Action::Previous => vec![Key::Char('a')],
      Action::ToggleShuffle => vec![Key::Char('s')],
      Action::CycleLoopStatus => vec![Key::Char('r')],
      Action::IncreaseVolume => vec![Key::Char(']')],
      Action::DecreaseVolume => vec![Key::Char('[')],
      Action::SeekForwards => vec![Key::Right],
      Action::SeekBackwards => vec![Key::Left],
      Action::Search => vec![Key::Char('/')],
      Action::PaneDown => vec![Key::Char('j'), Key::Down],
      Action::PaneUp => vec![Key::Char('k'), Key::Up],
      Action::PaneTop => vec![Key::Char('g'), Key::Home],
      Action::PaneBottom => vec![Key::Char('G'), Key::End],
      Action::ToggleLike => vec![Key::Char('f')],
      Action::AddToQueue => vec![Key::Char('c')],
      Action::AddToPlaylist => vec![Key::Char('C')],
      Action::PaneOpen => vec![Key::Char('l'), Key::Char('\n')],
      Action::PaneTracks => vec![Key::Char('o')],
      Action::RemoveFromPlaylist => vec![Key::Char('d')],
      Action::PaneParent => vec![Key::Backspace, Key::Char('-')],
      Action::PaneShowHidden => vec![Key::Char('H')],
      Action::PaneHide => vec![Key::Char('h')],
      Action::PanePin => vec![Key::Char('p')],
    }
  }

  fn should_exit(&self) -> bool {
    matches!(self, Action::Quit)
  }
}

fn key_to_string(key: Key) -> String {
  match key {
    Key::Char(' ') => "Space".to_string(),
    Key::Char('\n') => "Enter".to_string(),
    Key::Char('\t') => "Tab".to_string(),
    Key::Char(c) => c.to_string(),
    Key::Ctrl(c) => format!("Ctrl+{}", c),
    Key::Alt(c) => format!("Alt+{}", c),
    Key::F(n) => format!("F{}", n),
    Key::Left => "Left".to_string(),
    Key::Right => "Right".to_string(),
    Key::Up => "Up".to_string(),
    Key::Down => "Down".to_string(),
    Key::Backspace => "Backspace".to_string(),
    Key::Delete => "Delete".to_string(),
    Key::Insert => "Insert".to_string(),
    Key::Home => "Home".to_string(),
    Key::End => "End".to_string(),
    Key::PageUp => "PageUp".to_string(),
    Key::PageDown => "PageDown".to_string(),
    Key::BackTab => "Shift+Tab".to_string(),
    Key::Esc => "Esc".to_string(),
    _ => "?".to_string(),
  }
}

fn parse_key(text: &str) -> Option<Key> {
  let mut chars = text.chars();
  let first = chars.next()?;
  if chars.next().is_none() {
    return Some(Key::Char(first));
  }
  if let Some(rest) = text.strip_prefix("Ctrl+") {
    return rest.chars().next().filter(|_| rest.chars().count() == 1).map(Key::Ctrl);
  }
  if let Some(rest) = text.strip_prefix("Alt+") {
    return rest.chars().next().filter(|_| rest.chars().count() == 1).map(Key::Alt);
  }
  if let Some(rest) = text.strip_prefix('F') {
    if let Ok(n) = rest.parse::<u8>() {
      return Some(Key::F(n));
    }
  }
  match text {
    "Space" => Some(Key::Char(' ')),
    "Enter" => Some(Key::Char('\n')),
    "Tab" => Some(Key::Char('\t')),
    "Left" => Some(Key::Left),
    "Right" => Some(Key::Right),
    "Up" => Some(Key::Up),
    "Down" => Some(Key::Down),
    "Backspace" => Some(Key::Backspace),
    "Delete" => Some(Key::Delete),
    "Insert" => Some(Key::Insert),
    "Home" => Some(Key::Home),
    "End" => Some(Key::End),
    "PageUp" => Some(Key::PageUp),
    "PageDown" => Some(Key::PageDown),
    "Shift+Tab" => Some(Key::BackTab),
    _ => None,
  }
}

#[derive(Serialize, Deserialize, Default)]
struct HotkeysFile {
  #[serde(default)]
  keys: BTreeMap<String, Vec<String>>,
}

struct Bindings {
  keys: Vec<(Action, Vec<Key>)>,
}

impl Bindings {
  fn defaults() -> Bindings {
    Bindings { keys: ACTIONS.iter().map(|action| (*action, action.default_keys())).collect() }
  }

  fn load() -> (Bindings, Option<String>) {
    let mut bindings = Bindings::defaults();
    let text = match fs::read_to_string("hotkeys.toml") {
      Ok(text) => text,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (bindings, None),
      Err(e) => return (bindings, Some(format_error("Reading hotkeys.toml", e))),
    };
    let file = match toml::from_str::<HotkeysFile>(&text) {
      Ok(file) => file,
      Err(e) => return (bindings, Some(format_error("Reading hotkeys.toml", e))),
    };
    let mut error = None;
    for (action, keys) in bindings.keys.iter_mut() {
      if let Some(names) = file.keys.get(&format!("{:?}", action)) {
        let parsed: Vec<Key> = names.iter().filter_map(|name| parse_key(name)).collect();
        if parsed.len() != names.len() {
          error = Some(format!("hotkeys.toml: unknown key for {:?}", action));
        }
        *keys = parsed;
      }
    }
    (bindings, error)
  }

  fn save(&self) -> Result<(), String> {
    let file = HotkeysFile {
      keys: self
        .keys
        .iter()
        .map(|(action, keys)| {
          (format!("{:?}", action), keys.iter().map(|key| key_to_string(*key)).collect())
        })
        .collect(),
    };
    let text = toml::to_string(&file).map_err(|e| format_error("Saving hotkeys.toml", e))?;
    fs::write("hotkeys.toml", text).map_err(|e| format_error("Saving hotkeys.toml", e))
  }

  fn display(&self, action: Action) -> String {
    self
      .keys
      .iter()
      .find(|(other, _)| *other == action)
      .map(|(_, keys)| {
        keys
          .iter()
          .map(|key| {
            let name = key_to_string(*key);
            if name.chars().count() > 1 { name.to_lowercase() } else { name }
          })
          .collect::<Vec<_>>()
          .join("/")
      })
      .filter(|text| !text.is_empty())
      .unwrap_or_else(|| "unbound".to_string())
  }

  fn lookup(&self, key: Key) -> Option<Action> {
    let matches: Vec<Action> = self
      .keys
      .iter()
      .filter(|(_, keys)| keys.contains(&key))
      .map(|(action, _)| *action)
      .collect();
    matches.iter().copied().find(|action| action.is_pane()).or_else(|| matches.into_iter().next())
  }

  fn assign(&mut self, action: Action, key: Key) {
    for (other, keys) in self.keys.iter_mut() {
      if *other == action {
        *keys = vec![key];
      } else if other.is_pane() == action.is_pane() {
        keys.retain(|existing| *existing != key);
      }
    }
  }
}

enum Menu {
  Closed,
  Main(usize),
  Hotkeys { cursor: usize, capturing: bool },
  Settings(usize),
  AddToPlaylist { cursor: usize, uri: String, title: String },
}

const MENU_ITEMS: &[&str] = &["resume", "hotkeys", "settings", "quit"];
const SETTING_COUNT: usize = 6;
const MAX_QUEUE: usize = 20;
const MAX_VISUALIZER: usize = 14;
const MIN_VISUALIZER_WIDTH: usize = 6;
const MAX_VISUALIZER_WIDTH: usize = 200;
const VISUALIZER_WIDTH_STEP: i32 = (BAR_WIDTH + 1) as i32;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
enum RepeatSetting {
  #[default]
  Off,
  All,
  One,
}

impl RepeatSetting {
  fn state(&self) -> RepeatState {
    match self {
      RepeatSetting::Off => RepeatState::Off,
      RepeatSetting::All => RepeatState::Context,
      RepeatSetting::One => RepeatState::Track,
    }
  }

  fn cycle(&self, delta: i32) -> RepeatSetting {
    let all = [RepeatSetting::Off, RepeatSetting::All, RepeatSetting::One];
    let index = all.iter().position(|other| other == self).unwrap_or(0) as i32;
    all[(index + delta).rem_euclid(3) as usize]
  }

  fn name(&self) -> &'static str {
    match self {
      RepeatSetting::Off => "off",
      RepeatSetting::All => "all",
      RepeatSetting::One => "one",
    }
  }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
  queue_length: usize,
  visualizer_height: usize,
  visualizer_width: usize,
  show_cover: bool,
  default_shuffle: bool,
  default_repeat: RepeatSetting,
}

impl Default for Settings {
  fn default() -> Settings {
    Settings { queue_length: 6, visualizer_height: 0, visualizer_width: 0, show_cover: true, default_shuffle: false, default_repeat: RepeatSetting::Off }
  }
}

impl Settings {
  fn load() -> (Settings, Option<String>) {
    match fs::read_to_string("settings.toml") {
      Ok(text) => match toml::from_str::<Settings>(&text) {
        Ok(settings) => (settings.clamped(), None),
        Err(e) => (Settings::default(), Some(format_error("Reading settings.toml", e))),
      },
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
      Err(e) => (Settings::default(), Some(format_error("Reading settings.toml", e))),
    }
  }

  fn clamped(mut self) -> Settings {
    self.queue_length = self.queue_length.min(MAX_QUEUE);
    self.visualizer_height = self.visualizer_height.min(MAX_VISUALIZER);
    self.visualizer_width = self.visualizer_width.min(MAX_VISUALIZER_WIDTH);
    self
  }

  fn save(&self) -> Result<(), String> {
    let text = toml::to_string(self).map_err(|e| format_error("Saving settings.toml", e))?;
    fs::write("settings.toml", text).map_err(|e| format_error("Saving settings.toml", e))
  }

  fn adjust(&mut self, index: usize, delta: i32) {
    match index {
      0 => self.queue_length = (self.queue_length as i32 + delta).clamp(0, MAX_QUEUE as i32) as usize,
      1 => {
        let value = if self.visualizer_height == 0 { 1 } else { self.visualizer_height as i32 };
        let next = (value + delta).clamp(1, MAX_VISUALIZER as i32) as usize;
        self.visualizer_height = if next < 2 { 0 } else { next };
      }
      2 => {
        self.visualizer_width = match (self.visualizer_width, delta > 0) {
          (0, true) => 30,
          (0, false) => 0,
          (width, _) => {
            let next = width as i32 + delta.signum() * VISUALIZER_WIDTH_STEP;
            if next < MIN_VISUALIZER_WIDTH as i32 { 0 } else { next.min(MAX_VISUALIZER_WIDTH as i32) as usize }
          }
        };
      }
      3 => self.show_cover = !self.show_cover,
      4 => self.default_shuffle = !self.default_shuffle,
      _ => self.default_repeat = self.default_repeat.cycle(delta),
    }
  }

  fn defaults(&self) -> Defaults {
    (self.default_shuffle, self.default_repeat.state())
  }

  fn rows(&self) -> Vec<(String, String)> {
    let queue = if self.queue_length == 0 { "hidden".to_string() } else { self.queue_length.to_string() };
    let visualizer = if self.visualizer_height == 0 { "auto".to_string() } else { self.visualizer_height.to_string() };
    vec![
      ("songs in queue".to_string(), format!("< {} >", queue)),
      ("visualizer height".to_string(), format!("< {} >", visualizer)),
      ("visualizer width".to_string(), format!("< {} >", if self.visualizer_width == 0 { "auto".to_string() } else { format!("{} cols", self.visualizer_width) })),
      ("album cover".to_string(), format!("< {} >", if self.show_cover { "on" } else { "off" })),
      ("default shuffle".to_string(), format!("< {} >", if self.default_shuffle { "on" } else { "off" })),
      ("default repeat".to_string(), format!("< {} >", self.default_repeat.name())),
    ]
  }
}

#[derive(Clone, PartialEq)]
struct PlaybackState {
  has_context: bool,
  device_name: Option<String>,
  is_playing: bool,
  shuffle: bool,
  repeat: RepeatState,
  volume: Option<u32>,
  title: Option<String>,
  artists: Option<String>,
  album: Option<String>,
  uri: Option<String>,
  queue: Vec<ItemInfo>,
  cover_url: Option<String>,
  progress: Duration,
  length: Option<Duration>,
}

impl PlaybackState {
  fn same_view(&self, other: &PlaybackState) -> bool {
    let mut a = self.clone();
    let mut b = other.clone();
    a.progress = Duration::ZERO;
    b.progress = Duration::ZERO;
    a == b
  }

  fn new() -> PlaybackState {
    PlaybackState {
      has_context: false,
      device_name: None,
      is_playing: false,
      shuffle: false,
      repeat: RepeatState::Off,
      volume: None,
      title: None,
      artists: None,
      album: None,
      uri: None,
      queue: Vec::new(),
      cover_url: None,
      progress: Duration::ZERO,
      length: None,
    }
  }
}

fn own_spotifyd_pids() -> Vec<i32> {
  let uid = unsafe { libc::getuid() }.to_string();
  Command::new("pgrep")
    .args(["-x", "-u", &uid, "spotifyd"])
    .output()
    .map(|output| {
      String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| line.trim().parse().ok()).collect()
    })
    .unwrap_or_default()
}

fn stop_all_spotifyd() {
  let pids = own_spotifyd_pids();
  if pids.is_empty() {
    return;
  }
  println!("stopping {} running spotifyd...", pids.len());
  for pid in &pids {
    unsafe { libc::kill(*pid, libc::SIGTERM) };
  }
  for _ in 0..30 {
    if own_spotifyd_pids().is_empty() {
      return;
    }
    std::thread::sleep(Duration::from_millis(100));
  }
  for pid in own_spotifyd_pids() {
    unsafe { libc::kill(pid, libc::SIGKILL) };
  }
}

async fn restart_spotifyd(spotify: &AuthCodeSpotify) -> (Option<Child>, Option<String>) {
  stop_all_spotifyd();
  println!("starting spotifyd...");
  let child = start_spotifyd(true);
  if child.is_none() {
    println!("could not start spotifyd, continuing without it");
    return (None, None);
  }
  for second in 1..=SPOTIFYD_START_SECS {
    let processes = spotifyd_processes();
    let ready = spotify
      .device()
      .await
      .ok()
      .and_then(|devices| devices.into_iter().find(|d| d.name == DEVICE_NAME && is_live_device(d, &processes)));
    if let Some(device) = ready {
      println!("\rspotifyd is ready                                   ");
      return (child, device.id);
    }
    print!("\rwaiting for spotifyd to come online... {}s", second);
    std::io::stdout().flush().ok();
    tokio::time::sleep(Duration::from_secs(1)).await;
  }
  println!("\rspotifyd is taking a while, opening the app anyway      ");
  (child, None)
}

fn start_spotifyd(interactive: bool) -> Option<Child> {
  let cache = std::env::var("HOME").ok().map(|h| format!("{h}/.cache/spotifyd/oauth"));
  let logged_in = cache
    .and_then(|dir| fs::read_dir(dir).ok())
    .is_some_and(|mut entries| entries.next().is_some());
  if !logged_in && !interactive {
    return None;
  }
  if !logged_in {
    println!("spotifyd is not logged in yet, opening the spotify login for it...");
    let ok = Command::new("spotifyd").arg("authenticate").status().is_ok_and(|s| s.success());
    if !ok {
      eprintln!("spotifyd login failed, continuing without it");
      return None;
    }
  }

  let mut command = Command::new("spotifyd");
  command
    .args(["--no-daemon", "--use-mpris", "--device-name", DEVICE_NAME])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
  unsafe {
    command.pre_exec(|| {
      libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
      Ok(())
    });
  }
  match command.spawn() {
    Ok(child) => Some(child),
    Err(e) => {
      eprintln!("could not start spotifyd: {e}");
      None
    }
  }
}

fn format_error(what: &str, error: impl std::fmt::Display) -> String {
  format!("{} failed: {}", what.to_lowercase(), error).replace(['\r', '\n'], " ")
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
  Playlists,
  Search,
}

#[derive(Serialize, Deserialize, Default)]
struct PinsFile {
  #[serde(default)]
  pinned: Vec<String>,
  #[serde(default)]
  hidden_playlists: Vec<String>,
}

enum Entry {
  Playlist(usize),
  Result(usize),
}

struct TracksView {
  id: PlaylistId<'static>,
  name: String,
  tracks: Option<Result<Vec<ItemInfo>, String>>,
  cursor: usize,
  scroll: usize,
}

impl TracksView {
  fn count(&self) -> usize {
    match &self.tracks {
      Some(Ok(tracks)) => tracks.len(),
      _ => 0,
    }
  }

  fn move_cursor(&mut self, delta: i32) {
    let last = self.count().saturating_sub(1) as i32;
    self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
  }

  fn fit(&mut self, rows: usize) {
    self.cursor = self.cursor.min(self.count().saturating_sub(1));
    if rows == 0 {
      return;
    }
    if self.cursor < self.scroll {
      self.scroll = self.cursor;
    } else if self.cursor >= self.scroll + rows {
      self.scroll = self.cursor + 1 - rows;
    }
  }
}

enum Activation {
  Playlist(PlaylistId<'static>),
  Track(TrackId<'static>, ItemInfo),
}

struct PlaylistPane {
  results_more: bool,
  results_offset: u32,
  results_loading: bool,
  editable: HashSet<String>,
  totals: HashMap<String, u32>,
  results: Option<Vec<(String, TrackId<'static>, ItemInfo)>>,
  pinned: Vec<String>,
  hidden_playlists: Vec<String>,
  show_hidden: bool,
  recent: Vec<String>,
  playlists: Vec<(String, PlaylistId<'static>)>,
  cursor: usize,
  scroll: usize,
}

impl PlaylistPane {
  fn new(pins: PinsFile) -> PlaylistPane {
    PlaylistPane {
      results_more: false,
      results_offset: 0,
      results_loading: false,
      editable: HashSet::new(),
      totals: HashMap::new(),
      results: None,
      pinned: pins.pinned,
      hidden_playlists: pins.hidden_playlists,
      show_hidden: false,
      recent: Vec::new(),
      playlists: Vec::new(),
      cursor: 0,
      scroll: 0,
    }
  }

  fn set_playlists(&mut self, playlists: Vec<(String, PlaylistId<'static>)>, recent: Vec<String>) {
    self.playlists = playlists;
    self.recent = recent;
    self.resort();
  }

  fn resort(&mut self) {
    let pinned = &self.pinned;
    let recent = &self.recent;
    self.playlists.sort_by_key(|(name, id)| {
      (
        pinned.iter().position(|pinned| pinned == name).unwrap_or(usize::MAX),
        recent.iter().position(|uri| *uri == id.uri()).unwrap_or(usize::MAX),
      )
    });
  }

  fn toggle_hide(&mut self) -> bool {
    let Some(Entry::Playlist(index)) = self.entries().into_iter().nth(self.cursor) else {
      return false;
    };
    let name = self.playlists[index].0.clone();
    match self.hidden_playlists.iter().position(|hidden| *hidden == name) {
      Some(position) => {
        self.hidden_playlists.remove(position);
      }
      None => self.hidden_playlists.push(name),
    }
    true
  }

  fn toggle_pin(&mut self) -> bool {
    let Some(Entry::Playlist(index)) = self.entries().into_iter().nth(self.cursor) else {
      return false;
    };
    let (name, id) = self.playlists[index].clone();
    match self.pinned.iter().position(|pinned| *pinned == name) {
      Some(position) => {
        self.pinned.remove(position);
      }
      None => self.pinned.push(name),
    }
    self.resort();
    let moved = self.playlists.iter().position(|(_, other)| *other == id);
    self.cursor = self
      .entries()
      .iter()
      .position(|entry| matches!(entry, Entry::Playlist(i) if Some(*i) == moved))
      .unwrap_or(0);
    true
  }

  fn entries(&self) -> Vec<Entry> {
    if let Some(results) = &self.results {
      return (0..results.len()).map(Entry::Result).collect();
    }
    (0..self.playlists.len())
      .filter(|index| self.show_hidden || !self.hidden_playlists.contains(&self.playlists[*index].0))
      .map(Entry::Playlist)
      .collect()
  }

  fn move_cursor(&mut self, delta: i32) {
    let last = self.entries().len().saturating_sub(1) as i32;
    self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
  }

  fn fit(&mut self, rows: usize) {
    self.cursor = self.cursor.min(self.entries().len().saturating_sub(1));
    if rows == 0 {
      return;
    }
    if self.cursor < self.scroll {
      self.scroll = self.cursor;
    } else if self.cursor >= self.scroll + rows {
      self.scroll = self.cursor + 1 - rows;
    }
  }

  fn activate(&mut self) -> Option<Activation> {
    match self.entries().get(self.cursor)? {
      Entry::Playlist(index) => Some(Activation::Playlist(self.playlists[*index].1.clone())),
      Entry::Result(index) => {
        let (_, id, info) = self.results.as_ref()?[*index].clone();
        Some(Activation::Track(id, info))
      }
    }
  }

  fn parent(&mut self) {
    if self.results.take().is_some() {
      self.cursor = 0;
      self.scroll = 0;
    }
  }
}

fn load_pins() -> Result<PinsFile, String> {
  match fs::read_to_string("pins.toml") {
    Ok(text) => toml::from_str::<PinsFile>(&text)
      .map_err(|e| format_error("Reading pins.toml", e)),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PinsFile::default()),
    Err(e) => Err(format_error("Reading pins.toml", e)),
  }
}

#[derive(Clone)]
enum PlayerCommand {
  Pause,
  Resume,
  Stop,
  Next,
  Previous,
  Shuffle(bool),
  Repeat(RepeatState),
  Seek(chrono::Duration),
  Volume(u8),
  PlayTrack(TrackId<'static>, Defaults),
  PlayPlaylist(PlaylistId<'static>, Defaults, u32, Option<(String, usize)>, bool),
}

type Defaults = (bool, RepeatState);
type PlaylistData = (Vec<(String, PlaylistId<'static>)>, Vec<String>, HashMap<String, u32>, HashSet<String>);

impl PlayerCommand {
  fn name(&self) -> &'static str {
    match self {
      PlayerCommand::Pause => "Pause",
      PlayerCommand::Resume => "Play",
      PlayerCommand::Stop => "Stop",
      PlayerCommand::Next => "Next",
      PlayerCommand::Previous => "Previous",
      PlayerCommand::Shuffle(_) => "Shuffle",
      PlayerCommand::Repeat(_) => "Repeat",
      PlayerCommand::Seek(_) => "Seek",
      PlayerCommand::Volume(_) => "Volume",
      PlayerCommand::PlayTrack(..) | PlayerCommand::PlayPlaylist(..) => "Play",
    }
  }
}

enum Message {
  Playback(PlaybackState),
  Status(Option<String>),
  Playlists(Result<PlaylistData, String>),
  Results(u64, Result<SearchPage, String>),
  MoreResults(u64, Result<SearchPage, String>),
  NeedSpotifyd,
  DeviceListed(bool),
  Liked(Vec<(String, bool)>),
  PlaylistContents(String, HashSet<String>),
  LikeFailed(String, bool, String),
  Notice(String),
  LocalTrack(ItemInfo),
  PlaylistTracks(String, Result<Vec<ItemInfo>, String>),
  Cover(String, Option<(RgbImage, Palette)>),
}

#[derive(Clone, PartialEq)]
struct ItemInfo {
  title: String,
  artists: String,
  album: String,
  uri: Option<String>,
  length: Duration,
  cover_url: Option<String>,
}

fn pick_cover_json(images: &serde_json::Value) -> Option<String> {
  images
    .as_array()?
    .iter()
    .filter_map(|image| Some((image["url"].as_str()?, image["width"].as_i64().unwrap_or(0))))
    .min_by_key(|(_, width)| (width - 300).abs())
    .map(|(url, _)| url.to_string())
}

fn item_info(item: PlayableItem) -> Option<ItemInfo> {
  match item {
    PlayableItem::Track(track) => Some(ItemInfo {
      title: track.name.clone(),
      artists: join_artists(&track.artists),
      album: track.album.name.clone(),
      uri: track.id.as_ref().map(|id| id.uri()),
      length: track.duration.to_std().unwrap_or_default(),
      cover_url: pick_cover(&track.album.images),
    }),
    PlayableItem::Episode(episode) => Some(ItemInfo {
      title: episode.name.clone(),
      artists: episode.show.name.clone(),
      album: episode.show.name.clone(),
      uri: Some(episode.id.uri()),
      length: episode.duration.to_std().unwrap_or_default(),
      cover_url: pick_cover(&episode.images),
    }),
    PlayableItem::Unknown(value) => {
      let is_episode = value["type"] == "episode";
      let artists = if is_episode {
        value["show"]["name"].as_str().unwrap_or("").to_string()
      } else {
        value["artists"]
          .as_array()?
          .iter()
          .filter_map(|artist| artist["name"].as_str())
          .collect::<Vec<_>>()
          .join(" + ")
      };
      let (album, images) = if is_episode {
        (artists.clone(), &value["images"])
      } else {
        (value["album"]["name"].as_str().unwrap_or("").to_string(), &value["album"]["images"])
      };
      Some(ItemInfo {
        title: value["name"].as_str()?.to_string(),
        artists,
        album,
        uri: value["uri"].as_str().map(String::from),
        length: Duration::from_millis(value["duration_ms"].as_u64().unwrap_or(0)),
        cover_url: pick_cover_json(images),
      })
    }
  }
}

fn pick_cover(images: &[rspotify::model::Image]) -> Option<String> {
  images
    .iter()
    .min_by_key(|image| (image.width.unwrap_or(0) as i64 - 300).abs())
    .map(|image| image.url.clone())
}

async fn fetch_cover(url: String) -> Option<(RgbImage, Palette)> {
  let bytes = reqwest::get(url).await.ok()?.bytes().await.ok()?;
  tokio::task::spawn_blocking(move || {
    let image = image::load_from_memory(&bytes).ok()?.thumbnail(640, 640).to_rgb8();
    let palette = palette_from(&image);
    Some((image, palette))
  })
  .await
  .ok()?
}

type Rgb = (u8, u8, u8);

const DEFAULT_ACCENT: Rgb = (180, 190, 254);
const KITTY_CLEAR: &str = "\x1b_Ga=d,d=A,q=2\x1b\\";

#[derive(Clone, Copy, PartialEq)]
enum Graphics {
  Kitty,
  TrueColor,
  Ansi256,
  Plain,
}

impl Graphics {
  fn detect() -> Graphics {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let (term, program, colorterm) = (var("TERM"), var("TERM_PROGRAM"), var("COLORTERM"));
    let in_multiplexer = !var("TMUX").is_empty() || term.starts_with("screen");
    let kitty_protocol = term.contains("kitty")
      || term.contains("ghostty")
      || !var("KITTY_WINDOW_ID").is_empty()
      || program == "ghostty"
      || program == "WezTerm";
    if kitty_protocol && !in_multiplexer {
      Graphics::Kitty
    } else if colorterm.contains("truecolor") || colorterm.contains("24bit") {
      Graphics::TrueColor
    } else if term.contains("256color") {
      Graphics::Ansi256
    } else {
      Graphics::Plain
    }
  }

  fn fg(&self, (r, g, b): Rgb) -> String {
    match self {
      Graphics::Kitty | Graphics::TrueColor => format!("\x1b[38;2;{};{};{}m", r, g, b),
      Graphics::Ansi256 => format!("{}", color::Fg(color::AnsiValue::rgb(r / 52, g / 52, b / 52))),
      Graphics::Plain => String::new(),
    }
  }

  fn bg(&self, (r, g, b): Rgb) -> String {
    match self {
      Graphics::Kitty | Graphics::TrueColor => format!("\x1b[48;2;{};{};{}m", r, g, b),
      Graphics::Ansi256 => format!("{}", color::Bg(color::AnsiValue::rgb(r / 52, g / 52, b / 52))),
      Graphics::Plain => String::new(),
    }
  }
}

const LEVEL_BINS: usize = 6;

#[derive(Clone)]
struct Palette {
  accent: Rgb,
  colors: Vec<Rgb>,
  ramp: Vec<Rgb>,
}

fn blend(stops: &[Rgb], t: f32) -> Rgb {
  let last = stops.len() - 1;
  if last == 0 {
    return stops[0];
  }
  let scaled = t.clamp(0.0, 1.0) * last as f32;
  let index = (scaled.floor() as usize).min(last - 1);
  let fraction = scaled - index as f32;
  let (a, b) = (stops[index], stops[index + 1]);
  let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * fraction) as u8;
  (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

fn luma((r, g, b): Rgb) -> f32 {
  0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

fn make_ramp(colors: &[Rgb]) -> Vec<Rgb> {
  let mut sorted = colors.to_vec();
  sorted.sort_by(|a, b| luma(*a).total_cmp(&luma(*b)));
  let brightest = *sorted.last().unwrap();
  let lighten = |c: u8| (c as f32 + (255.0 - c as f32) * 0.35) as u8;
  let mut stops = vec![scale_color(sorted[0], 0.55)];
  stops.extend(sorted.iter().copied());
  stops.push((lighten(brightest.0), lighten(brightest.1), lighten(brightest.2)));
  (0..LEVEL_BINS).map(|i| blend(&stops, i as f32 / (LEVEL_BINS - 1) as f32)).collect()
}

impl Palette {
  fn default_palette() -> Palette {
    let colors = vec![(137, 180, 250), (180, 190, 254), (203, 166, 247), (245, 189, 230), (250, 179, 135)];
    Palette { accent: DEFAULT_ACCENT, ramp: make_ramp(&colors), colors }
  }

  fn color(&self, index: usize) -> Rgb {
    self.colors[index % self.colors.len()]
  }

  fn gradient(&self, t: f32) -> Rgb {
    blend(&self.colors, t)
  }

  fn level_color(&self, level: f32) -> Rgb {
    let bin = ((level.clamp(0.0, 1.0) * LEVEL_BINS as f32) as usize).min(LEVEL_BINS - 1);
    self.ramp[bin]
  }
}

fn scale_color((r, g, b): Rgb, factor: f32) -> Rgb {
  ((r as f32 * factor).min(255.0) as u8, (g as f32 * factor).min(255.0) as u8, (b as f32 * factor).min(255.0) as u8)
}

fn hue_of((r, g, b): Rgb) -> f32 {
  let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
  let high = r.max(g).max(b);
  let low = r.min(g).min(b);
  let delta = high - low;
  if delta < 1e-6 {
    return 0.0;
  }
  let hue = if high == r {
    ((g - b) / delta).rem_euclid(6.0)
  } else if high == g {
    (b - r) / delta + 2.0
  } else {
    (r - g) / delta + 4.0
  };
  hue * 60.0
}

fn palette_from(image: &RgbImage) -> Palette {
  let small = resize(image, 48, 48, FilterType::Triangle);
  let mut buckets: HashMap<u16, (u32, [u32; 3])> = HashMap::new();
  for pixel in small.pixels() {
    let [r, g, b] = pixel.0;
    let key = ((r as u16 >> 4) << 8) | ((g as u16 >> 4) << 4) | (b as u16 >> 4);
    let entry = buckets.entry(key).or_insert((0, [0; 3]));
    entry.0 += 1;
    entry.1[0] += r as u32;
    entry.1[1] += g as u32;
    entry.1[2] += b as u32;
  }
  let mut candidates: Vec<(f32, Rgb)> = buckets
    .into_values()
    .filter_map(|(count, sum)| {
      let color = ((sum[0] / count) as u8, (sum[1] / count) as u8, (sum[2] / count) as u8);
      let high = color.0.max(color.1).max(color.2) as f32;
      let low = color.0.min(color.1).min(color.2) as f32;
      if high < 40.0 {
        return None;
      }
      let saturation = (high - low) / high;
      Some((count as f32 * (0.3 + saturation), color))
    })
    .collect();
  candidates.sort_by(|a, b| b.0.total_cmp(&a.0));

  let lift = |color: Rgb| {
    let high = color.0.max(color.1).max(color.2).max(1) as f32;
    if high < 160.0 { scale_color(color, 160.0 / high) } else { color }
  };
  let distance = |a: Rgb, b: Rgb| {
    let d = |x: u8, y: u8| (x as f32 - y as f32).powi(2);
    (d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)).sqrt()
  };
  let mut chosen: Vec<Rgb> = Vec::new();
  for (_, color) in candidates {
    let color = lift(color);
    if chosen.iter().all(|other| distance(*other, color) > 70.0) {
      chosen.push(color);
      if chosen.len() == 5 {
        break;
      }
    }
  }
  let Some(&accent) = chosen.first() else {
    return Palette::default_palette();
  };
  chosen.sort_by(|a, b| hue_of(*a).total_cmp(&hue_of(*b)));
  let mut step = 1;
  while chosen.len() < 4 {
    let base = chosen[step % chosen.len()];
    let mixed = |c: u8| (c as f32 + (255.0 - c as f32) * (0.25 * step as f32).min(0.8)) as u8;
    chosen.push((mixed(base.0), mixed(base.1), mixed(base.2)));
    step += 1;
  }
  Palette { accent, ramp: make_ramp(&chosen), colors: chosen }
}

fn kitty_png_b64(image: &RgbImage, px: u32) -> String {
  let resized = resize(image, px, px, FilterType::Lanczos3);
  let mut png = Vec::new();
  resized.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok();
  base64::engine::general_purpose::STANDARD.encode(png)
}

fn kitty_place(b64: &str, cols: usize, rows: usize) -> String {
  let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
  let mut out = String::new();
  for (i, chunk) in chunks.iter().enumerate() {
    let more = (i + 1 < chunks.len()) as u8;
    let data = std::str::from_utf8(chunk).unwrap_or("");
    if i == 0 {
      write!(out, "\x1b_Gf=100,a=T,i=1,p=1,c={cols},r={rows},C=1,q=2,m={more};{data}\x1b\\").unwrap();
    } else {
      write!(out, "\x1b_Gm={more};{data}\x1b\\").unwrap();
    }
  }
  out
}

fn die_with_parent(command: &mut Command) -> &mut Command {
  unsafe {
    command.pre_exec(|| {
      libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
      Ok(())
    });
  }
  command
}

const RATE: u32 = 22050;
const FFT_SIZE: usize = 1024;
const LOW_HZ: f32 = 50.0;
const HIGH_HZ: f32 = 10000.0;
const RANGE_DB: f32 = 45.0;
const MIN_GAIN_DB: f32 = 80.0;
const RECHECK: Duration = Duration::from_secs(2);
const STALE: Duration = Duration::from_millis(400);
const BAR_WIDTH: usize = 2;

struct Visualizer {
  samples: Arc<Mutex<VecDeque<f32>>>,
  last_audio: Arc<AtomicU64>,
  epoch: Instant,
  alive: Arc<AtomicBool>,
  child: Arc<Mutex<Option<Child>>>,
  fft: Arc<dyn Fft<f32>>,
  window: Vec<f32>,
  levels: Vec<f32>,
  gain: f32,
}

impl Visualizer {
  fn start() -> Visualizer {
    let samples = Arc::new(Mutex::new(VecDeque::from(vec![0.0; FFT_SIZE])));
    let alive = Arc::new(AtomicBool::new(true));
    let child = Arc::new(Mutex::new(None));
    let last_audio = Arc::new(AtomicU64::new(0));
    let epoch = Instant::now();
    let capture = Capture {
      samples: samples.clone(),
      last_audio: last_audio.clone(),
      epoch,
      alive: alive.clone(),
      child: child.clone(),
    };
    std::thread::spawn(move || capture.run());
    let window = (0..FFT_SIZE)
      .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
      .collect();
    Visualizer {
      samples,
      last_audio,
      epoch,
      alive,
      child,
      fft: FftPlanner::new().plan_fft_forward(FFT_SIZE),
      window,
      levels: Vec::new(),
      gain: 0.0,
    }
  }

  fn close(&self) {
    self.alive.store(false, Ordering::SeqCst);
    if let Some(mut child) = self.child.lock().unwrap().take() {
      child.kill().ok();
      child.wait().ok();
    }
  }

  fn update(&mut self, bars: usize, active: bool) -> &[f32] {
    if self.levels.len() != bars {
      self.levels = vec![0.0; bars];
    }
    let fresh = self.epoch.elapsed().as_millis() as u64
      <= self.last_audio.load(Ordering::Relaxed) + STALE.as_millis() as u64;
    if !active || !fresh || bars == 0 {
      self.levels.iter_mut().for_each(|level| *level *= 0.8);
      return &self.levels;
    }
    let mut buffer: Vec<Complex<f32>> = {
      let samples = self.samples.lock().unwrap();
      samples.iter().zip(&self.window).map(|(v, w)| Complex::new(v * w, 0.0)).collect()
    };
    self.fft.process(&mut buffer);
    let magnitudes: Vec<f32> = buffer[..FFT_SIZE / 2].iter().map(|c| c.norm()).collect();

    let bin_hz = RATE as f32 / FFT_SIZE as f32;
    let ratio = (HIGH_HZ / LOW_HZ).powf(1.0 / bars as f32);
    let mut raw = Vec::with_capacity(bars);
    let mut previous_high = 1usize;
    for band in 0..bars {
      let low = previous_high
        .max((LOW_HZ * ratio.powi(band as i32) / bin_hz) as usize)
        .min(FFT_SIZE / 2 - 1);
      let high = (low + 1)
        .max((LOW_HZ * ratio.powi(band as i32 + 1) / bin_hz) as usize)
        .min(FFT_SIZE / 2);
      previous_high = high;
      let peak = magnitudes[low..high].iter().cloned().fold(0.0f32, f32::max);
      let centre = ((low * high) as f32).sqrt() * bin_hz;
      raw.push(20.0 * (peak + 1e-9).log10() + 3.0 * (centre / LOW_HZ).log2());
    }
    let peak = raw.iter().cloned().fold(f32::MIN, f32::max);
    self.gain = peak.max(self.gain - 0.05).max(MIN_GAIN_DB);
    for (level, value) in self.levels.iter_mut().zip(raw) {
      let target = ((value - (self.gain - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0).powf(1.6);
      *level = if target > *level { target } else { *level * 0.82 + target * 0.18 };
    }
    &self.levels
  }
}

impl Drop for Visualizer {
  fn drop(&mut self) {
    self.close();
  }
}

struct PipewireNode {
  serial: String,
  running: bool,
  linked: bool,
}

fn spotify_nodes() -> Option<Vec<PipewireNode>> {
  let out = Command::new("pw-dump").stderr(Stdio::null()).output().ok()?;
  let dump: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
  let objects = dump.as_array()?;
  let linked: HashSet<u64> = objects
    .iter()
    .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Link")))
    .filter_map(|o| o["info"]["output-node-id"].as_u64())
    .collect();
  let mut nodes = Vec::new();
  for object in objects {
    let info = &object["info"];
    let props = &info["props"];
    if props["media.class"] != "Stream/Output/Audio" {
      continue;
    }
    let name = format!(
      "{}{}",
      props["application.name"].as_str().unwrap_or(""),
      props["application.process.binary"].as_str().unwrap_or("")
    )
    .to_lowercase();
    if !name.contains("spotify") {
      continue;
    }
    let serial = match &props["object.serial"] {
      serde_json::Value::Number(n) => n.to_string(),
      serde_json::Value::String(s) => s.clone(),
      _ => continue,
    };
    nodes.push(PipewireNode {
      serial,
      running: info["state"] == "running",
      linked: object["id"].as_u64().is_some_and(|id| linked.contains(&id)),
    });
  }
  Some(nodes)
}

fn best_node(nodes: &[PipewireNode]) -> Option<&str> {
  nodes
    .iter()
    .find(|n| n.running)
    .or_else(|| nodes.iter().find(|n| n.linked))
    .map(|n| n.serial.as_str())
}

struct Capture {
  samples: Arc<Mutex<VecDeque<f32>>>,
  last_audio: Arc<AtomicU64>,
  epoch: Instant,
  alive: Arc<AtomicBool>,
  child: Arc<Mutex<Option<Child>>>,
}

impl Capture {
  fn alive(&self) -> bool {
    self.alive.load(Ordering::SeqCst)
  }

  fn run(self) {
    while self.alive() {
      let target = spotify_nodes().and_then(|nodes| best_node(&nodes).map(String::from));
      match target {
        Some(node) => self.capture(&node),
        None => std::thread::sleep(Duration::from_secs(1)),
      }
      if let Some(mut child) = self.child.lock().unwrap().take() {
        child.kill().ok();
        child.wait().ok();
      }
    }
  }

  fn capture(&self, node: &str) {
    let spawned = die_with_parent(&mut Command::new("pw-record"))
      .args(["--target", node, "--rate", &RATE.to_string(), "--channels", "1", "--format", "s16"])
      .args(["-P", "{ node.dont-reconnect = true }", "--raw", "-"])
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .spawn();
    let Ok(mut child) = spawned else {
      std::thread::sleep(Duration::from_secs(1));
      return;
    };
    let Some(stdout) = child.stdout.take() else {
      return;
    };
    *self.child.lock().unwrap() = Some(child);
    let fd = stdout.as_raw_fd();

    let mut buffer = [0u8; 4096];
    let mut carry: Option<u8> = None;
    let mut last_check = Instant::now();
    while self.alive() {
      let mut pollfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
      let ready = unsafe { libc::poll(&mut pollfd, 1, 200) };
      if ready > 0 {
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count <= 0 {
          return;
        }
        let mut bytes: Vec<u8> = carry.take().into_iter().collect();
        bytes.extend_from_slice(&buffer[..count as usize]);
        if bytes.len() % 2 == 1 {
          carry = bytes.pop();
        }
        {
          let mut samples = self.samples.lock().unwrap();
          samples.extend(bytes.chunks_exact(2).map(|p| i16::from_le_bytes([p[0], p[1]]) as f32));
          let excess = samples.len().saturating_sub(FFT_SIZE);
          samples.drain(..excess);
        }
        self.last_audio.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
      } else if ready < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
        return;
      }
      if last_check.elapsed() >= RECHECK {
        last_check = Instant::now();
        if let Some(nodes) = spotify_nodes() {
          match best_node(&nodes) {
            Some(best) if best != node => return,
            None if !nodes.iter().any(|n| n.serial == node) => return,
            _ => {}
          }
        }
      }
    }
  }
}

fn join_artists(artists: &[rspotify::model::SimplifiedArtist]) -> String {
  artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(" + ")
}

fn hostname() -> String {
  fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_string()
}

struct SpotifydProcess {
  device_name: String,
}

fn spotifyd_processes() -> Vec<SpotifydProcess> {
  let Ok(output) = Command::new("pgrep").args(["-x", "spotifyd"]).output() else {
    return Vec::new();
  };
  String::from_utf8_lossy(&output.stdout)
    .lines()
    .filter_map(|pid| {
      let output = Command::new("ps").args(["-o", "args=", "-p", pid.trim()]).output().ok()?;
      let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
      let words: Vec<&str> = text.split_whitespace().collect();
      let device_name = words
        .iter()
        .position(|word| *word == "--device-name" || *word == "-d")
        .and_then(|i| words.get(i + 1).map(|name| name.to_string()))
        .or_else(|| {
          words.iter().find_map(|word| word.strip_prefix("--device-name=").map(String::from))
        })
        .unwrap_or_else(hostname);
      (!words.is_empty()).then_some(SpotifydProcess { device_name })
    })
    .collect()
}

fn local_device_names() -> Vec<String> {
  let mut names = vec![DEVICE_NAME.to_string(), hostname()];
  for process in spotifyd_processes() {
    if !names.contains(&process.device_name) {
      names.push(process.device_name);
    }
  }
  names
}

fn is_live_device(device: &rspotify::model::Device, processes: &[SpotifydProcess]) -> bool {
  if device.id.is_none() || device.is_restricted {
    return false;
  }
  let local = device.name == DEVICE_NAME || device.name == hostname();
  !local || processes.iter().any(|process| process.device_name == device.name)
}

fn is_local_device(name: Option<&str>, local_names: &[String]) -> bool {
  name.is_some_and(|name| local_names.iter().any(|local| local == name))
}

fn local_skip(player: &mpris::Player, forwards: bool, tx: &Sender<Message>) -> bool {
  let before = player.get_metadata().ok().and_then(|m| m.track_id()).map(String::from);
  let skipped = if forwards { player.next() } else { player.previous() };
  if skipped.is_err() {
    return false;
  }
  for _ in 0..20 {
    std::thread::sleep(Duration::from_millis(30));
    let Ok(metadata) = player.get_metadata() else {
      continue;
    };
    let Some(id) = metadata.track_id().map(String::from) else {
      continue;
    };
    if Some(&id) == before.as_ref() {
      continue;
    }
    let title = metadata.title().unwrap_or("").to_string();
    if title.is_empty() {
      continue;
    }
    let info = ItemInfo {
      title,
      artists: metadata.artists().map(|artists| artists.join(" + ")).unwrap_or_default(),
      album: metadata.album_name().unwrap_or("").to_string(),
      uri: id.rsplit('/').next().map(|track| format!("spotify:track:{}", track)),
      length: metadata.length().unwrap_or_default(),
      cover_url: metadata.art_url().map(String::from),
    };
    tx.send(Message::LocalTrack(info)).ok();
    break;
  }
  true
}

fn local_execute(command: &PlayerCommand, tx: &Sender<Message>) -> bool {
  let Ok(finder) = mpris::PlayerFinder::new() else {
    return false;
  };
  let Ok(players) = finder.find_all() else {
    return false;
  };
  let Some(player) = players.into_iter().find(|p| p.identity().to_lowercase().contains("spotifyd")) else {
    return false;
  };
  match command {
    PlayerCommand::Pause => player.pause().is_ok(),
    PlayerCommand::Resume => player.play().is_ok(),
    PlayerCommand::Next => local_skip(&player, true, tx),
    PlayerCommand::Previous => local_skip(&player, false, tx),
    PlayerCommand::Shuffle(state) => player.set_shuffle(*state).is_ok(),
    PlayerCommand::Repeat(state) => player
      .set_loop_status(match state {
        RepeatState::Off => mpris::LoopStatus::None,
        RepeatState::Context => mpris::LoopStatus::Playlist,
        RepeatState::Track => mpris::LoopStatus::Track,
      })
      .is_ok(),
    PlayerCommand::Volume(volume) => player.set_volume(*volume as f64 / 100.0).is_ok(),
    PlayerCommand::Seek(position) => {
      let position = position.to_std().unwrap_or_default();
      player
        .get_metadata()
        .ok()
        .and_then(|metadata| metadata.track_id())
        .is_some_and(|id| player.set_position(id, &position).is_ok())
    }
    PlayerCommand::Stop | PlayerCommand::PlayTrack(..) | PlayerCommand::PlayPlaylist(..) => false,
  }
}

async fn apply_defaults(
  spotify: &AuthCodeSpotify,
  device: Option<&str>,
  (shuffle, repeat): Defaults,
  started: bool,
  tx: &Sender<Message>,
  notify: &Notify,
) {
  if !started {
    return;
  }
  tx.send(Message::Status(None)).ok();
  notify.notify_one();
  for attempt in 0..4 {
    let (shuffled, repeated) = tokio::join!(spotify.shuffle(shuffle, device), spotify.repeat(repeat, device));
    if shuffled.is_ok() && repeated.is_ok() {
      break;
    }
    tokio::time::sleep(Duration::from_millis(400 * (attempt + 1))).await;
  }
  notify.notify_one();
}

async fn resolve_device(
  spotify: &AuthCodeSpotify,
  has_context: bool,
  wait: bool,
  local_names: &[String],
  tx: &Sender<Message>,
) -> Result<Option<String>, String> {
  if has_context {
    return Ok(None);
  }
  let attempts = DEVICE_WAIT_SECS;
  for attempt in 0..attempts {
    let devices = spotify.device().await.map_err(|e| format_error("Listing devices", e))?;
    let processes = spotifyd_processes();
    let usable = |d: &&rspotify::model::Device| is_live_device(d, &processes);
    let found = devices
      .iter()
      .filter(usable)
      .find(|d| local_names.iter().any(|name| *name == d.name))
      .or_else(|| devices.iter().find(usable))
      .map(|d| d.id.clone());
    if let Some(id) = found {
      return Ok(id);
    }
    if attempt == 0 {
      if !wait {
        tx.send(Message::NeedSpotifyd).ok();
      }
      tx.send(Message::Status(Some("waiting for spotifyd to show up as a device...".to_string()))).ok();
    }
    if attempt + 1 < attempts {
      tokio::time::sleep(Duration::from_secs(1)).await;
    }
  }
  Err("no spotify device found. open spotify somewhere or start spotifyd, then try again".to_string())
}

async fn dispatch(
  spotify: &AuthCodeSpotify,
  command: PlayerCommand,
  device: Option<&str>,
  by_position: bool,
  tx: &Sender<Message>,
  notify: &Notify,
) -> rspotify::ClientResult<()> {
  match command {
    PlayerCommand::Pause => spotify.pause_playback(device).await,
    PlayerCommand::Resume => spotify.resume_playback(device, None).await,
    PlayerCommand::Stop => match spotify.pause_playback(device).await {
      Ok(()) => spotify.seek_track(chrono::Duration::zero(), device).await,
      error => error,
    },
    PlayerCommand::Next => spotify.next_track(device).await,
    PlayerCommand::Previous => spotify.previous_track(device).await,
    PlayerCommand::Shuffle(state) => spotify.shuffle(state, device).await,
    PlayerCommand::Repeat(state) => spotify.repeat(state, device).await,
    PlayerCommand::Seek(position) => spotify.seek_track(position, device).await,
    PlayerCommand::Volume(volume) => spotify.volume(volume, device).await,
    PlayerCommand::PlayTrack(id, defaults) => {
      let started = spotify.start_uris_playback([PlayableId::Track(id)], device, None, None).await;
      apply_defaults(spotify, device, defaults, started.is_ok(), tx, notify).await;
      started
    }
    PlayerCommand::PlayPlaylist(id, defaults, total, start, unshuffle) => {
      if start.is_some() && unshuffle {
        spotify.shuffle(false, device).await.ok();
      }
      let offset = match start {
        Some((_, index)) if by_position => {
          Some(rspotify::model::Offset::Position(chrono::Duration::milliseconds(index as i64)))
        }
        Some((uri, _)) => Some(rspotify::model::Offset::Uri(uri)),
        None => (defaults.0 && total > 1).then(|| {
          let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
          rspotify::model::Offset::Position(chrono::Duration::milliseconds((nanos % total) as i64))
        }),
      };
      let started = spotify.start_context_playback(PlayContextId::Playlist(id), device, offset, None).await;
      apply_defaults(spotify, device, defaults, started.is_ok(), tx, notify).await;
      started
    }
  }
}

async fn execute(
  spotify: &AuthCodeSpotify,
  has_context: bool,
  wait: bool,
  local_names: &[String],
  known_device: Option<&str>,
  tx: &Sender<Message>,
  notify: &Notify,
  command: PlayerCommand,
) -> Result<(), String> {
  let name = command.name();
  let device = match known_device {
    Some(id) if !has_context => Some(id.to_string()),
    _ => resolve_device(spotify, has_context, wait, local_names, tx).await?,
  };
  let mut result = dispatch(spotify, command.clone(), device.as_deref(), false, tx, notify).await;
  if result.as_ref().is_err_and(|e| e.to_string().contains("404")) {
    if let Ok(Some(fresh)) = resolve_device(spotify, false, wait, local_names, tx).await {
      result = dispatch(spotify, command.clone(), Some(&fresh), false, tx, notify).await;
      if result.is_err() && matches!(command, PlayerCommand::PlayPlaylist(_, _, _, Some(_), _)) {
        result = dispatch(spotify, command, Some(&fresh), true, tx, notify).await;
      }
    }
  }
  result.map_err(|e| format_error(name, e))
}

async fn fetch_playback(spotify: &AuthCodeSpotify) -> Result<PlaybackState, String> {
  let context = spotify
    .current_playback(None, None::<Vec<&AdditionalType>>)
    .await
    .map_err(|e| format_error("could not read playback state, request", e))?;
  let mut state = PlaybackState::new();
  let Some(context) = context else {
    return Ok(state);
  };

  state.has_context = true;
  state.device_name = Some(context.device.name.clone());
  state.is_playing = context.is_playing;
  state.shuffle = context.shuffle_state;
  state.repeat = context.repeat_state;
  state.volume = context.device.volume_percent;
  state.progress = context.progress.map(|p| p.to_std().unwrap_or_default()).unwrap_or_default();
  if let Some(info) = context.item.and_then(item_info) {
    state.title = Some(info.title);
    state.artists = Some(info.artists);
    state.album = Some(info.album);
    state.uri = info.uri;
    state.length = Some(info.length);
    state.cover_url = info.cover_url;
  }
  Ok(state)
}

async fn fetch_queue(spotify: &AuthCodeSpotify) -> Vec<ItemInfo> {
  spotify
    .current_user_queue()
    .await
    .map(|queue| queue.queue.into_iter().take(30).filter_map(item_info).collect())
    .unwrap_or_default()
}

async fn fetch_playlist_tracks(spotify: &AuthCodeSpotify, id: &PlaylistId<'static>) -> Result<Vec<ItemInfo>, String> {
  let url = format!("playlists/{}/items", id.id());
  let mut tracks = Vec::new();
  let mut offset = 0;
  while offset < MAX_PLAYLIST_TRACKS {
    let offset_text = offset.to_string();
    let query = HashMap::from([("limit", "50"), ("offset", offset_text.as_str())]);
    let body = spotify.api_get(&url, &query).await.map_err(|e| format_error("Loading songs", e))?;
    let page: serde_json::Value = serde_json::from_str(&body).map_err(|e| format_error("Reading songs", e))?;
    let items = page["items"].as_array().cloned().unwrap_or_default();
    tracks.extend(items.into_iter().filter(|entry| !entry["item"].is_null()).filter_map(|entry| item_info(PlayableItem::Unknown(entry["item"].clone()))));
    if !page["next"].is_string() {
      break;
    }
    offset += 50;
  }
  Ok(tracks)
}

fn encode_uri(uri: &str) -> String {
  uri.replace(':', "%3A")
}

async fn fetch_liked(spotify: &AuthCodeSpotify, uris: Vec<String>) -> Vec<(String, bool)> {
  let mut found = Vec::new();
  for chunk in uris.chunks(40) {
    let joined = chunk.iter().map(|uri| encode_uri(uri)).collect::<Vec<_>>().join("%2C");
    let url = format!("me/library/contains?uris={}", joined);
    let Ok(body) = spotify.api_get(&url, &HashMap::new()).await else {
      continue;
    };
    let Ok(values) = serde_json::from_str::<Vec<bool>>(&body) else {
      continue;
    };
    found.extend(chunk.iter().cloned().zip(values));
  }
  found
}

async fn set_liked(spotify: &AuthCodeSpotify, uri: &str, like: bool) -> Result<(), String> {
  let url = format!("me/library?uris={}", encode_uri(uri));
  let result = if like {
    spotify.api_put(&url, &serde_json::json!({})).await
  } else {
    spotify.api_delete(&url, &serde_json::json!({})).await
  };
  result.map(|_| ()).map_err(|e| format_error(if like { "like" } else { "unlike" }, e))
}

async fn add_to_playlist(spotify: &AuthCodeSpotify, playlist: &PlaylistId<'static>, uri: &str) -> Result<(), String> {
  let body = serde_json::json!({ "uris": [uri] });
  let first = spotify.api_post(&format!("playlists/{}/items", playlist.id()), &body).await;
  match first {
    Ok(_) => Ok(()),
    Err(error) => spotify
      .api_post(&format!("playlists/{}/tracks", playlist.id()), &body)
      .await
      .map(|_| ())
      .map_err(|_| format_error("add to playlist", error)),
  }
}

async fn fetch_playlist_uris(spotify: &AuthCodeSpotify, id: &PlaylistId<'static>) -> HashSet<String> {
  let url = format!("playlists/{}/items", id.id());
  let mut uris = HashSet::new();
  let mut offset = 0;
  while offset < 1000 {
    let offset_text = offset.to_string();
    let query = HashMap::from([("limit", "50"), ("offset", offset_text.as_str()), ("fields", "items(item(uri)),next")]);
    let Ok(body) = spotify.api_get(&url, &query).await else {
      break;
    };
    let Ok(page) = serde_json::from_str::<serde_json::Value>(&body) else {
      break;
    };
    for entry in page["items"].as_array().cloned().unwrap_or_default() {
      if let Some(uri) = entry["item"]["uri"].as_str() {
        uris.insert(uri.to_string());
      }
    }
    if !page["next"].is_string() {
      break;
    }
    offset += 50;
  }
  uris
}

async fn remove_from_playlist(spotify: &AuthCodeSpotify, playlist: &PlaylistId<'static>, uri: &str) -> Result<(), String> {
  let first = spotify
    .api_delete(&format!("playlists/{}/items", playlist.id()), &serde_json::json!({ "items": [{ "uri": uri }] }))
    .await;
  match first {
    Ok(_) => Ok(()),
    Err(error) => spotify
      .api_delete(&format!("playlists/{}/tracks", playlist.id()), &serde_json::json!({ "tracks": [{ "uri": uri }] }))
      .await
      .map(|_| ())
      .map_err(|_| format_error("remove from playlist", error)),
  }
}

async fn fetch_playlists(spotify: &AuthCodeSpotify) -> Result<PlaylistData, String> {
  let playlists = spotify
    .current_user_playlists()
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format_error("Loading playlists", e))?;

  let mut recent: Vec<String> = Vec::new();
  if let Ok(page) = spotify.current_user_recently_played(Some(50), None).await {
    for context in page.items.into_iter().filter_map(|item| item.context) {
      if !recent.contains(&context.uri) {
        recent.push(context.uri);
      }
    }
  }
  let totals = playlists.iter().map(|p| (p.id.uri(), p.items.total)).collect();
  let me = spotify
    .api_get("me", &HashMap::new())
    .await
    .ok()
    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
    .and_then(|value| value["id"].as_str().map(String::from))
    .unwrap_or_default();
  let editable = playlists
    .iter()
    .filter(|p| p.collaborative || p.owner.id.id() == me)
    .map(|p| p.id.uri())
    .collect();
  Ok((playlists.into_iter().map(|p| (p.name, p.id)).collect(), recent, totals, editable))
}

struct SearchPage {
  items: Vec<(String, TrackId<'static>, ItemInfo)>,
  has_more: bool,
  next_offset: u32,
}

async fn search_page(spotify: &AuthCodeSpotify, query: &str, offset: u32) -> Result<SearchPage, String> {
  match spotify.search(query, SearchType::Track, None, None, Some(SEARCH_PAGE), Some(offset)).await {
    Ok(SearchResult::Tracks(page)) => {
      let next_offset = offset + page.items.len() as u32;
      let has_more = page.items.len() >= SEARCH_PAGE as usize;
      let items = page
        .items
        .into_iter()
        .filter_map(|t| {
          let label = format!("{} - {}", join_artists(&t.artists), t.name);
          let id = t.id.clone()?;
          let info = item_info(PlayableItem::Track(t))?;
          Some((label, id, info))
        })
        .collect();
      Ok(SearchPage { items, has_more, next_offset })
    }
    Ok(_) => Ok(SearchPage { items: Vec::new(), has_more: false, next_offset: offset }),
    Err(e) => Err(format_error("Search", e)),
  }
}

async fn search_until(spotify: &AuthCodeSpotify, query: &str, want: usize) -> Result<SearchPage, String> {
  let mut result = search_page(spotify, query, 0).await?;
  while result.has_more && result.items.len() < want && result.items.len() < MAX_RESULTS {
    let page = search_page(spotify, query, result.next_offset).await?;
    result.items.extend(page.items);
    result.has_more = page.has_more;
    result.next_offset = page.next_offset;
  }
  Ok(result)
}

fn spawn_poller(
  spotify: AuthCodeSpotify,
  tx: Sender<Message>,
  notify: Arc<Notify>,
  refresh_queue: Arc<AtomicBool>,
  owns_spotifyd: Arc<AtomicBool>,
) {
  async fn poll_once(
    spotify: &AuthCodeSpotify,
    tx: &Sender<Message>,
    state: &mut PlaybackState,
    force_queue: bool,
  ) -> bool {
    match fetch_playback(spotify).await {
      Ok(mut next) => {
        let changed = next.uri != state.uri;
        next.queue = match state.queue.first() {
          Some(first) if changed && first.uri == next.uri => state.queue[1..].to_vec(),
          _ => state.queue.clone(),
        };
        *state = next;
        if tx.send(Message::Playback(state.clone())).is_err() {
          return false;
        }
        if (changed || force_queue) && state.uri.is_some() {
          state.queue = fetch_queue(spotify).await;
          return tx.send(Message::Playback(state.clone())).is_ok();
        }
        true
      }
      Err(message) => tx.send(Message::Status(Some(message))).is_ok(),
    }
  }

  tokio::spawn(async move {
    let mut state = PlaybackState::new();
    let mut rounds = 0u32;
    loop {
      rounds += 1;
      if rounds % 8 == 0 && owns_spotifyd.load(Ordering::Relaxed) {
        let listed = spotify
          .device()
          .await
          .map(|devices| devices.iter().any(|d| d.name == DEVICE_NAME))
          .unwrap_or(true);
        if tx.send(Message::DeviceListed(listed)).is_err() {
          return;
        }
      }
      let force = refresh_queue.load(Ordering::Relaxed);
      if !poll_once(&spotify, &tx, &mut state, force).await {
        return;
      }
      let burst = tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(POLL_INTERVAL)) => false,
        _ = notify.notified() => true,
      };
      if burst {
        for delay in [150, 300, 600, 900, 1200] {
          tokio::time::sleep(Duration::from_millis(delay)).await;
          let force = refresh_queue.load(Ordering::Relaxed);
          if !poll_once(&spotify, &tx, &mut state, force).await {
            return;
          }
        }
        refresh_queue.store(false, Ordering::Relaxed);
      }
    }
  });
}

struct App {
  bindings: Bindings,
  settings: Settings,
  local_names: Vec<String>,
  local_device_id: Option<String>,
  menu: Menu,
  pane: PlaylistPane,
  tracks_view: Option<TracksView>,
  focus: Focus,
  query: String,
  search_id: u64,
  visualizer: Visualizer,
  levels: Vec<f32>,
  cover: Option<RgbImage>,
  cover_url: Option<String>,
  covers: HashMap<String, (RgbImage, Palette)>,
  covers_inflight: HashSet<String>,
  cover_scaled: Option<((usize, usize), RgbImage)>,
  graphics: Graphics,
  palette: Palette,
  cell_px: (f32, f32),
  cell_aspect: f32,
  prev_rows: Vec<String>,
  placed: Option<Placement>,
  kitty_cache: Option<(String, u32, String)>,
  last_tick: Instant,
  contents: HashMap<String, HashSet<String>>,
  contents_inflight: HashSet<String>,
  pending_remove: Option<(String, Instant)>,
  liked: HashSet<String>,
  liked_checked: HashSet<String>,
  notice: Option<(String, Instant)>,
  optimistic_until: Instant,
  expect_uri: Option<String>,
  history: Vec<ItemInfo>,
  defaults_applied: bool,
  events: termion::input::Events<termion::AsyncReader>,
  screen: Screen,
  spotify: AuthCodeSpotify,
  state: PlaybackState,
  size: (u16, u16),
  fetched_at: Instant,
  status: Option<String>,
  spotifyd: Option<Child>,
  tx: Sender<Message>,
  rx: Receiver<Message>,
  notify: Arc<Notify>,
  refresh_queue: Arc<AtomicBool>,
  owns_spotifyd: Arc<AtomicBool>,
  last_watch: Instant,
  device_seen: bool,
  device_missing: u32,
}

impl Drop for App {
  fn drop(&mut self) {
    if self.graphics == Graphics::Kitty {
      write!(self.screen, "{}", KITTY_CLEAR).ok();
    }
    write!(self.screen, "{}", termion::cursor::Show).ok();
    self.screen.flush().ok();
    if let Some(mut child) = self.spotifyd.take() {
      child.kill().ok();
      child.wait().ok();
    }
  }
}

impl App {
  fn main_loop(&mut self) {
    loop {
      let mut dirty = false;
      while let Some(event) = self.next_event() {
        dirty = true;
        if self.handle_event(event) {
          return;
        }
      }
      self.watch_spotifyd();
      if self.notice.as_ref().is_some_and(|(_, shown)| shown.elapsed() > Duration::from_secs(3)) {
        self.notice = None;
        dirty = true;
      }
      dirty |= self.apply_messages();
      self.refresh(dirty);
      std::thread::sleep(Duration::from_millis(FRAME_INTERVAL));
    }
  }

  fn next_event(&mut self) -> Option<Event> {
    self.events.next().and_then(|result| result.ok())
  }

  fn load_more_results(&mut self) {
    let Some(results) = &self.pane.results else {
      return;
    };
    if !self.pane.results_more || self.pane.results_loading || self.pane.cursor + 6 < results.len() {
      return;
    }
    self.pane.results_loading = true;
    let (spotify, tx, query, offset, id) =
      (self.spotify.clone(), self.tx.clone(), self.query.clone(), self.pane.results_offset, self.search_id);
    tokio::spawn(async move {
      tx.send(Message::MoreResults(id, search_page(&spotify, &query, offset).await)).ok();
    });
  }

  fn check_liked(&mut self, uris: Vec<String>) {
    let fresh: Vec<String> = uris.into_iter().filter(|uri| self.liked_checked.insert(uri.clone())).collect();
    if fresh.is_empty() {
      return;
    }
    let (spotify, tx) = (self.spotify.clone(), self.tx.clone());
    tokio::spawn(async move {
      let found = fetch_liked(&spotify, fresh).await;
      tx.send(Message::Liked(found)).ok();
    });
  }

  fn current_target(&self) -> Option<(String, TrackId<'static>, String)> {
    let from_info = |info: &ItemInfo| {
      let uri = info.uri.clone()?;
      let id = TrackId::from_uri(&uri).ok()?.into_static();
      Some((uri, id, format!("{} - {}", info.artists, info.title)))
    };
    if let Some(view) = &self.tracks_view {
      return match &view.tracks {
        Some(Ok(tracks)) => tracks.get(view.cursor).and_then(from_info),
        _ => None,
      };
    }
    if let Some(results) = &self.pane.results {
      let Some(Entry::Result(index)) = self.pane.entries().into_iter().nth(self.pane.cursor) else {
        return None;
      };
      return from_info(&results[index].2);
    }
    from_info(&self.current_item()?)
  }

  fn toggle_like(&mut self) {
    let Some((uri, _, title)) = self.current_target() else {
      return;
    };
    let like = !self.liked.contains(&uri);
    if like {
      self.liked.insert(uri.clone());
    } else {
      self.liked.remove(&uri);
    }
    self.liked_checked.insert(uri.clone());
    let (spotify, tx) = (self.spotify.clone(), self.tx.clone());
    tokio::spawn(async move {
      match set_liked(&spotify, &uri, like).await {
        Ok(()) => {
          let verb = if like { "liked" } else { "unliked" };
          tx.send(Message::Notice(format!("{} {}", verb, title))).ok();
        }
        Err(message) => {
          tx.send(Message::LikeFailed(uri, !like, message)).ok();
        }
      }
    });
  }

  fn add_to_queue(&mut self) {
    let Some((_, id, title)) = self.current_target() else {
      return;
    };
    let device = if self.state.has_context { None } else { self.local_device_id.clone() };
    let (spotify, tx, notify) = (self.spotify.clone(), self.tx.clone(), self.notify.clone());
    self.refresh_queue.store(true, Ordering::Relaxed);
    tokio::spawn(async move {
      match spotify.add_item_to_queue(PlayableId::Track(id), device.as_deref()).await {
        Ok(()) => {
          tx.send(Message::Notice(format!("added to queue: {}", title))).ok();
          notify.notify_one();
        }
        Err(e) => {
          tx.send(Message::Status(Some(format_error("add to queue", e)))).ok();
        }
      }
    });
  }

  fn open_playlist_picker(&mut self) {
    let Some((uri, _, title)) = self.current_target() else {
      return;
    };
    if !self.pane.playlists.iter().any(|(_, id)| self.pane.editable.contains(&id.uri())) {
      self.status = Some("no playlists you can add to".to_string());
      return;
    }
    let missing: Vec<PlaylistId<'static>> = self
      .editable_playlists()
      .into_iter()
      .map(|(_, id)| id)
      .filter(|id| !self.contents.contains_key(&id.uri()) && self.contents_inflight.insert(id.uri()))
      .collect();
    if !missing.is_empty() {
      let (spotify, tx) = (self.spotify.clone(), self.tx.clone());
      tokio::spawn(async move {
        for id in missing {
          let uris = fetch_playlist_uris(&spotify, &id).await;
          if tx.send(Message::PlaylistContents(id.uri(), uris)).is_err() {
            return;
          }
        }
      });
    }
    self.menu = Menu::AddToPlaylist { cursor: 0, uri, title };
  }

  fn set_picker_cursor(&mut self, value: usize) {
    if let Menu::AddToPlaylist { cursor, .. } = &mut self.menu {
      *cursor = value;
    }
  }

  fn confirm_picker(&mut self, cursor: usize) {
    let Some((name, id)) = self.editable_playlists().get(cursor).cloned() else {
      return;
    };
    let Menu::AddToPlaylist { uri, title, .. } = std::mem::replace(&mut self.menu, Menu::Closed) else {
      return;
    };
    let (spotify, tx) = (self.spotify.clone(), self.tx.clone());
    tokio::spawn(async move {
      match add_to_playlist(&spotify, &id, &uri).await {
        Ok(()) => {
          let mut known = HashSet::new();
          known.insert(uri.clone());
          tx.send(Message::PlaylistContents(format!("+{}", id.uri()), known)).ok();
          tx.send(Message::Notice(format!("added {} to {}", title, name))).ok();
        }
        Err(message) => {
          tx.send(Message::Status(Some(message))).ok();
        }
      }
    });
  }

  fn editable_playlists(&self) -> Vec<(String, PlaylistId<'static>)> {
    self
      .pane
      .playlists
      .iter()
      .filter(|(name, id)| {
        self.pane.editable.contains(&id.uri()) && (self.pane.show_hidden || !self.pane.hidden_playlists.contains(name))
      })
      .cloned()
      .collect()
  }

  fn restart_own_spotifyd(&mut self, message: &str) {
    if let Some(mut child) = self.spotifyd.take() {
      child.kill().ok();
      child.wait().ok();
    }
    self.status = Some(message.to_string());
    self.spotifyd = start_spotifyd(false);
    self.owns_spotifyd.store(self.spotifyd.is_some(), Ordering::Relaxed);
    self.device_seen = false;
    self.device_missing = 0;
  }

  fn watch_spotifyd(&mut self) {
    if self.last_watch.elapsed() < Duration::from_secs(2) {
      return;
    }
    self.last_watch = Instant::now();
    let stopped = match &mut self.spotifyd {
      Some(child) => matches!(child.try_wait(), Ok(Some(_))),
      None => false,
    };
    if stopped {
      self.restart_own_spotifyd("spotifyd stopped, restarting...");
    }
  }

  fn apply_messages(&mut self) -> bool {
    let mut dirty = false;
    while let Ok(message) = self.rx.try_recv() {
      let mut changed = true;
      match message {
        Message::Playback(state) => {
          if Instant::now() < self.optimistic_until {
            let ignore = match &self.expect_uri {
              Some(expected) => state.uri.as_ref() != Some(expected),
              None => state.uri == self.state.uri,
            };
            if ignore {
              if state.uri == self.state.uri && !state.queue.is_empty() && state.queue != self.state.queue {
                self.state.queue = state.queue.clone();
                dirty = true;
              }
              continue;
            }
            self.optimistic_until = Instant::now();
            self.expect_uri = None;
          }
          changed = !self.state.same_view(&state);
          if state.uri != self.state.uri {
            if let Some(previous) = self.current_item() {
              self.history.push(previous);
              if self.history.len() > 20 {
            self.history.remove(0);
          }
            }
            self.show_cover_for(state.cover_url.clone());
            if let Some(uri) = state.uri.clone() {
              self.check_liked(vec![uri]);
            }
          }
          for item in state.queue.iter().take(2) {
            if let Some(url) = item.cover_url.clone() {
              self.request_cover(url);
            }
          }
          if !self.defaults_applied && state.has_context {
            self.defaults_applied = true;
            let (shuffle, repeat) = self.settings.defaults();
            if state.shuffle != shuffle {
              self.run(PlayerCommand::Shuffle(shuffle));
            }
            if state.repeat != repeat {
              self.run(PlayerCommand::Repeat(repeat));
            }
          }
          self.state = state;
          self.fetched_at = Instant::now();
          if self.status.as_deref().is_some_and(|s| s.starts_with("could not read")) {
            self.status = None;
            changed = true;
          }
        }
        Message::Cover(url, result) => {
          self.covers_inflight.remove(&url);
          if let Some((image, palette)) = result {
            if self.covers.len() >= 8 {
              self.covers.clear();
            }
            if self.cover_url.as_ref() == Some(&url) {
              self.cover = Some(image.clone());
              self.palette = palette.clone();
              self.cover_scaled = None;
            }
            self.covers.insert(url, (image, palette));
          }
        }
        Message::PlaylistTracks(uri, result) => {
          if let Some(view) = &mut self.tracks_view {
            if view.id.uri() == uri {
              let uris: Vec<String> = match &result {
                Ok(tracks) => tracks.iter().take(200).filter_map(|info| info.uri.clone()).collect(),
                Err(_) => Vec::new(),
              };
              view.tracks = Some(result);
              self.check_liked(uris);
            }
          }
        }
        Message::LocalTrack(info) => {
          if self.state.uri != info.uri {
            if let Some(current) = self.current_item() {
              self.history.push(current);
              if self.history.len() > 20 {
                self.history.remove(0);
              }
            }
            if self.state.queue.first().is_some_and(|first| first.uri == info.uri) {
              self.state.queue.remove(0);
            }
          }
          self.show_item(info);
        }
        Message::PlaylistContents(uri, uris) => match uri.strip_prefix('+') {
          Some(playlist) => {
            self.contents.entry(playlist.to_string()).or_default().extend(uris);
          }
          None => {
            self.contents_inflight.remove(&uri);
            let mut merged = self.contents.remove(&uri).unwrap_or_default();
            merged.extend(uris);
            self.contents.insert(uri, merged);
          }
        },
        Message::Liked(found) => {
          for (uri, liked) in found {
            if liked {
              self.liked.insert(uri);
            } else {
              self.liked.remove(&uri);
            }
          }
        }
        Message::LikeFailed(uri, previous, message) => {
          if previous {
            self.liked.insert(uri);
          } else {
            self.liked.remove(&uri);
          }
          self.status = Some(message);
        }
        Message::Notice(text) => self.notice = Some((text, Instant::now())),
        Message::DeviceListed(listed) => {
          if listed {
            self.device_seen = true;
            self.device_missing = 0;
          } else if self.device_seen {
            self.device_missing += 1;
            if self.device_missing >= 3 {
              self.restart_own_spotifyd("spotifyd lost its connection, restarting...");
            }
          }
          changed = false;
        }
        Message::NeedSpotifyd => {
          if self.spotifyd.is_none() {
            self.spotifyd = start_spotifyd(false);
            self.owns_spotifyd.store(self.spotifyd.is_some(), Ordering::Relaxed);
            if self.spotifyd.is_none() {
              self.status = Some("no spotifyd running and none could be started (is it installed and logged in?)".to_string());
            }
          }
        }
        Message::Status(status) => self.status = status,
        Message::Playlists(Ok((playlists, recent, totals, editable))) => {
          self.pane.totals = totals;
          self.pane.editable = editable;
          self.pane.set_playlists(playlists, recent)
        }
        Message::Playlists(Err(message)) => self.status = Some(message),
        Message::Results(id, Ok(page)) if id == self.search_id => {
          let uris: Vec<String> = page.items.iter().filter_map(|(_, _, info)| info.uri.clone()).collect();
          self.check_liked(uris);
          self.pane.results = Some(page.items);
          self.pane.results_more = page.has_more;
          self.pane.results_offset = page.next_offset;
          self.pane.results_loading = false;
          self.pane.cursor = 0;
          self.pane.scroll = 0;
          self.status = None;
        }
        Message::Results(id, Err(message)) if id == self.search_id => self.status = Some(message),
        Message::Results(..) => changed = false,
        Message::MoreResults(id, Ok(page)) if id == self.search_id => {
          let uris: Vec<String> = page.items.iter().filter_map(|(_, _, info)| info.uri.clone()).collect();
          self.check_liked(uris);
          if let Some(results) = &mut self.pane.results {
            results.extend(page.items);
          }
          self.pane.results_more = page.has_more && self.pane.results.as_ref().is_some_and(|r| r.len() < MAX_RESULTS);
          self.pane.results_offset = page.next_offset;
          self.pane.results_loading = false;
        }
        Message::MoreResults(id, Err(message)) if id == self.search_id => {
          self.pane.results_loading = false;
          self.status = Some(message);
        }
        Message::MoreResults(..) => changed = false,
      }
      dirty |= changed;
    }
    dirty
  }

  fn handle_event(&mut self, event: Event) -> bool {
    match event {
      Event::Key(Key::Ctrl('c')) => true,
      Event::Key(key) if !matches!(self.menu, Menu::Closed) => self.handle_menu_key(key),
      Event::Key(key) if self.focus == Focus::Search => {
        self.handle_search_key(key);
        false
      }
      Event::Key(Key::Esc) => {
        self.menu = Menu::Main(0);
        false
      }
      Event::Key(key) => match self.bindings.lookup(key) {
        Some(action) => {
          self.perform_action(action);
          action.should_exit()
        }
        None => false,
      },
      Event::Mouse(MouseEvent::Press(button, x, y)) if matches!(self.menu, Menu::Closed) => {
        if !self.view_click(button, x as usize, y as usize) && !self.timeline_click(button, x as usize, y as usize) {
          self.pane_mouse(button, x as usize, y as usize);
        }
        false
      }
      _ => false,
    }
  }

  fn handle_search_key(&mut self, key: Key) {
    match key {
      Key::Esc => self.focus = Focus::Playlists,
      Key::Char('\n') => {
        self.focus = Focus::Playlists;
        if self.query.trim().is_empty() {
          self.pane.parent();
        } else {
          self.search_id += 1;
          let id = self.search_id;
          let want = layout(self.size, &self.bindings).map(|l| l.separator.saturating_sub(5)).unwrap_or(30).max(SEARCH_PAGE as usize);
          let (spotify, tx, query) = (self.spotify.clone(), self.tx.clone(), self.query.clone());
          tokio::spawn(async move {
            tx.send(Message::Results(id, search_until(&spotify, &query, want).await)).ok();
          });
        }
      }
      Key::Backspace => {
        self.query.pop();
      }
      Key::Ctrl('u') => self.query.clear(),
      Key::Char(c) if !c.is_control() => self.query.push(c),
      _ => {}
    }
  }

  fn request_cover(&mut self, url: String) {
    if self.covers.contains_key(&url) || !self.covers_inflight.insert(url.clone()) {
      return;
    }
    let tx = self.tx.clone();
    tokio::spawn(async move {
      let result = fetch_cover(url.clone()).await;
      tx.send(Message::Cover(url, result)).ok();
    });
  }

  fn change_setting(&mut self, index: usize, delta: i32) {
    self.settings.adjust(index, delta);
    self.status = self.settings.save().err();
  }

  fn handle_menu_key(&mut self, key: Key) -> bool {
    match self.menu {
      Menu::Closed => {}
      Menu::Main(cursor) => match key {
        Key::Esc => self.menu = Menu::Closed,
        Key::Up | Key::Char('k') => self.menu = Menu::Main(cursor.saturating_sub(1)),
        Key::Home | Key::Char('g') => self.menu = Menu::Main(0),
        Key::End | Key::Char('G') => self.menu = Menu::Main(MENU_ITEMS.len() - 1),
        Key::Down | Key::Char('j') => {
          self.menu = Menu::Main((cursor + 1).min(MENU_ITEMS.len() - 1))
        }
        Key::Char('\n') => match cursor {
          0 => self.menu = Menu::Closed,
          1 => self.menu = Menu::Hotkeys { cursor: 0, capturing: false },
          2 => self.menu = Menu::Settings(0),
          _ => return true,
        },
        _ => {}
      },
      Menu::AddToPlaylist { cursor, .. } => {
        let count = self.editable_playlists().len().max(1);
        match key {
          Key::Esc => self.menu = Menu::Closed,
          Key::Up | Key::Char('k') => self.set_picker_cursor(cursor.saturating_sub(1)),
          Key::Down | Key::Char('j') => self.set_picker_cursor((cursor + 1).min(count - 1)),
          Key::Home | Key::Char('g') => self.set_picker_cursor(0),
          Key::End | Key::Char('G') => self.set_picker_cursor(count - 1),
          Key::Char('\n') => self.confirm_picker(cursor),
          other => {
            if self.bindings.lookup(other) == Some(Action::PaneShowHidden) {
              self.pane.show_hidden = !self.pane.show_hidden;
              self.set_picker_cursor(0);
            }
          }
        }
      }
      Menu::Settings(cursor) => match key {
        Key::Esc => self.menu = Menu::Main(2),
        Key::Up | Key::Char('k') => self.menu = Menu::Settings(cursor.saturating_sub(1)),
        Key::Home | Key::Char('g') => self.menu = Menu::Settings(0),
        Key::End | Key::Char('G') => self.menu = Menu::Settings(SETTING_COUNT - 1),
        Key::Down | Key::Char('j') => self.menu = Menu::Settings((cursor + 1).min(SETTING_COUNT - 1)),
        Key::Left | Key::Char('h') => self.change_setting(cursor, -1),
        Key::Right | Key::Char('l') | Key::Char('\n') => self.change_setting(cursor, 1),
        _ => {}
      },
      Menu::Hotkeys { cursor, capturing: true } => match key {
        Key::Esc => self.menu = Menu::Hotkeys { cursor, capturing: false },
        Key::Ctrl('c') => {}
        _ => {
          self.bindings.assign(sorted_actions()[cursor], key);
          self.status = self.bindings.save().err();
          self.menu = Menu::Hotkeys { cursor, capturing: false };
        }
      },
      Menu::Hotkeys { cursor, capturing: false } => match key {
        Key::Esc => self.menu = Menu::Main(1),
        Key::Up | Key::Char('k') => {
          self.menu = Menu::Hotkeys { cursor: cursor.saturating_sub(1), capturing: false }
        }
        Key::Home | Key::Char('g') => self.menu = Menu::Hotkeys { cursor: 0, capturing: false },
        Key::End | Key::Char('G') => self.menu = Menu::Hotkeys { cursor: ACTIONS.len(), capturing: false },
        Key::Down | Key::Char('j') => {
          self.menu = Menu::Hotkeys { cursor: (cursor + 1).min(ACTIONS.len()), capturing: false }
        }
        Key::Char('\n') if cursor == ACTIONS.len() => {
          self.bindings = Bindings::defaults();
          self.status = self.bindings.save().err();
        }
        Key::Char('\n') => self.menu = Menu::Hotkeys { cursor, capturing: true },
        _ => {}
      },
    }
    false
  }

  fn timeline_click(&mut self, button: MouseButton, x: usize, y: usize) -> bool {
    let Some(player) = layout(self.size, &self.bindings).and_then(|l| player_layout(&l, self.cell_aspect, &self.settings)) else {
      return false;
    };
    let length = self.state.length.unwrap_or_default();
    let (bar_x, bar_len) = timeline_bar(&player, length);
    if y != player.timeline_y || x < bar_x || x >= bar_x + bar_len {
      return false;
    }
    if length.is_zero() {
      return true;
    }
    if button == MouseButton::Left && bar_len > 1 {
      let fraction = (x - bar_x) as f64 / (bar_len - 1) as f64;
      let target = length.mul_f64(fraction);
      self.state.progress = target;
      self.fetched_at = Instant::now();
      self.run(PlayerCommand::Seek(chrono::Duration::milliseconds(target.as_millis() as i64)));
    }
    true
  }

  fn pane_mouse(&mut self, button: MouseButton, x: usize, y: usize) {
    let Some(layout) = layout(self.size, &self.bindings) else {
      return;
    };
    if x < 2 || x > layout.left + 1 || y < 2 || y >= layout.separator {
      return;
    }
    if y <= 3 {
      if button == MouseButton::Left && y == 2 {
        self.focus = Focus::Search;
      }
      return;
    }
    self.focus = Focus::Playlists;
    match button {
      MouseButton::WheelUp => self.pane.move_cursor(-1),
      MouseButton::WheelDown => self.pane.move_cursor(1),
      MouseButton::Left if y == 4 => self.pane.parent(),
      MouseButton::Left => {
        let index = self.pane.scroll + y - 5;
        if index < self.pane.entries().len() {
          self.pane.cursor = index;
          self.activate_selected();
        }
      }
      _ => {}
    }
  }

  fn toggle_pin(&mut self) {
    if self.pane.toggle_pin() {
      self.save_pins();
    }
  }

  fn toggle_hide(&mut self) {
    if self.pane.toggle_hide() {
      self.save_pins();
    }
  }

  fn save_pins(&mut self) {
    let pins = PinsFile {
      pinned: self.pane.pinned.clone(),
      hidden_playlists: self.pane.hidden_playlists.clone(),
    };
    let text = toml::to_string(&pins).unwrap_or_default();
    self.status = fs::write("pins.toml", text).err().map(|e| format_error("Saving pins.toml", e));
  }

  fn move_selection(&mut self, delta: i32) {
    match &mut self.tracks_view {
      Some(view) => view.move_cursor(delta),
      None => self.pane.move_cursor(delta),
    }
  }

  fn toggle_tracks_view(&mut self) {
    if self.tracks_view.take().is_some() {
      return;
    }
    let Some(Entry::Playlist(index)) = self.pane.entries().into_iter().nth(self.pane.cursor) else {
      return;
    };
    let (name, id) = self.pane.playlists[index].clone();
    self.tracks_view = Some(TracksView { id: id.clone(), name, tracks: None, cursor: 0, scroll: 0 });
    let (spotify, tx) = (self.spotify.clone(), self.tx.clone());
    tokio::spawn(async move {
      let result = fetch_playlist_tracks(&spotify, &id).await;
      tx.send(Message::PlaylistTracks(id.uri(), result)).ok();
    });
  }

  fn play_from_view(&mut self) {
    let Some(view) = &self.tracks_view else {
      return;
    };
    let Some(Ok(tracks)) = &view.tracks else {
      return;
    };
    let Some(info) = tracks.get(view.cursor).cloned() else {
      return;
    };
    let index = view.cursor;
    let id = view.id.clone();
    self.tracks_view = None;
    let defaults = self.settings.defaults();
    let total = self.pane.totals.get(&id.uri()).copied().unwrap_or(0);
    let unshuffle = self.state.shuffle || !self.state.has_context;
    self.show_item(info.clone());
    let start = info.uri.clone().map(|uri| (uri, index));
    self.run(PlayerCommand::PlayPlaylist(id, defaults, total, start, unshuffle));
  }

  fn remove_from_view(&mut self) {
    let Some(view) = &self.tracks_view else {
      return;
    };
    let Some(Ok(tracks)) = &view.tracks else {
      return;
    };
    let Some(info) = tracks.get(view.cursor).cloned() else {
      return;
    };
    let Some(uri) = info.uri.clone() else {
      return;
    };
    if !self.pane.editable.contains(&view.id.uri()) {
      self.status = Some("you can't edit this playlist".to_string());
      return;
    }
    let confirmed = self.pending_remove.as_ref().is_some_and(|(pending, at)| *pending == uri && at.elapsed() < Duration::from_secs(4));
    let label = format!("{} - {}", info.artists, info.title);
    if !confirmed {
      self.pending_remove = Some((uri, Instant::now()));
      let key = self.bindings.display(Action::RemoveFromPlaylist);
      self.notice = Some((format!("press {} again to remove {}", key, label), Instant::now()));
      return;
    }
    self.pending_remove = None;
    let id = view.id.clone();
    let cursor = view.cursor;
    if let Some(view) = &mut self.tracks_view {
      if let Some(Ok(tracks)) = &mut view.tracks {
        tracks.remove(cursor);
      }
      view.cursor = cursor.min(view.count().saturating_sub(1));
    }
    if let Some(known) = self.contents.get_mut(&id.uri()) {
      known.remove(&uri);
    }
    let (spotify, tx) = (self.spotify.clone(), self.tx.clone());
    tokio::spawn(async move {
      match remove_from_playlist(&spotify, &id, &uri).await {
        Ok(()) => {
          tx.send(Message::Notice(format!("removed {}", label))).ok();
        }
        Err(message) => {
          tx.send(Message::Status(Some(message))).ok();
          let restored = fetch_playlist_tracks(&spotify, &id).await;
          tx.send(Message::PlaylistTracks(id.uri(), restored)).ok();
        }
      }
    });
  }

  fn view_click(&mut self, button: MouseButton, x: usize, y: usize) -> bool {
    let Some(layout) = layout(self.size, &self.bindings) else {
      return false;
    };
    let left = layout.left + 4;
    if self.tracks_view.is_none() || x < left || x + 2 > layout.width || y < 4 || y >= layout.separator {
      return false;
    }
    match button {
      MouseButton::WheelUp => self.move_selection(-1),
      MouseButton::WheelDown => self.move_selection(1),
      MouseButton::Left => {
        let target = self
          .tracks_view
          .as_ref()
          .map(|view| (view.scroll + (y - 4), view.count()))
          .filter(|(index, count)| index < count)
          .map(|(index, _)| index);
        if let Some(index) = target {
          if let Some(view) = &mut self.tracks_view {
            view.cursor = index;
          }
          self.play_from_view();
        }
      }
      _ => {}
    }
    true
  }

  fn activate_selected(&mut self) {
    let defaults = self.settings.defaults();
    match self.pane.activate() {
      Some(Activation::Playlist(id)) => {
        let total = self.pane.totals.get(&id.uri()).copied().unwrap_or(0);
        self.run(PlayerCommand::PlayPlaylist(id, defaults, total, None, false));
      }
      Some(Activation::Track(id, info)) => {
        self.show_item(info);
        self.run(PlayerCommand::PlayTrack(id, defaults));
      }
      None => {}
    }
  }

  fn current_item(&self) -> Option<ItemInfo> {
    Some(ItemInfo {
      title: self.state.title.clone()?,
      artists: self.state.artists.clone().unwrap_or_default(),
      album: self.state.album.clone().unwrap_or_default(),
      uri: self.state.uri.clone(),
      length: self.state.length.unwrap_or_default(),
      cover_url: self.state.cover_url.clone(),
    })
  }

  fn show_cover_for(&mut self, url: Option<String>) {
    self.cover_url = url;
    let cached = self.cover_url.as_ref().and_then(|url| self.covers.get(url)).cloned();
    match cached {
      Some((image, palette)) => {
        self.cover = Some(image);
        self.palette = palette;
      }
      None => {
        self.cover = None;
        if let Some(url) = self.cover_url.clone() {
          self.request_cover(url);
        }
      }
    }
    self.cover_scaled = None;
  }

  fn show_item(&mut self, item: ItemInfo) {
    self.state.has_context = true;
    self.state.is_playing = true;
    self.state.title = Some(item.title);
    self.state.artists = Some(item.artists);
    self.state.album = Some(item.album);
    self.state.length = Some(item.length);
    self.state.progress = Duration::ZERO;
    self.state.uri = item.uri.clone();
    self.state.cover_url = item.cover_url.clone();
    self.show_cover_for(item.cover_url);
    self.fetched_at = Instant::now();
    self.expect_uri = item.uri;
    self.optimistic_until = Instant::now() + Duration::from_millis(3000);
  }

  fn run(&mut self, command: PlayerCommand) {
    let local = self.state.has_context && is_local_device(self.state.device_name.as_deref(), &self.local_names);
    if !(local && matches!(command, PlayerCommand::Next | PlayerCommand::Previous)) {
      self.apply_optimistic(&command);
    }
    if matches!(
      command,
      PlayerCommand::Shuffle(_)
        | PlayerCommand::Next
        | PlayerCommand::Previous
        | PlayerCommand::PlayTrack(..)
        | PlayerCommand::PlayPlaylist(..)
    ) {
      self.refresh_queue.store(true, Ordering::Relaxed);
    }
    let (spotify, tx, notify) = (self.spotify.clone(), self.tx.clone(), self.notify.clone());
    let (has_context, wait) = (self.state.has_context, self.spotifyd.is_some());
    let local_names = self.local_names.clone();
    let known_device = self.local_device_id.clone();
    if matches!(command, PlayerCommand::PlayTrack(..) | PlayerCommand::PlayPlaylist(..)) {
      self.status = Some("starting playback...".to_string());
    }
    tokio::spawn(async move {
      let handled = local && {
        let (command, tx) = (command.clone(), tx.clone());
        tokio::task::spawn_blocking(move || local_execute(&command, &tx)).await.unwrap_or(false)
      };
      let result = if handled {
        Ok(())
      } else {
        execute(&spotify, has_context, wait, &local_names, known_device.as_deref(), &tx, &notify, command).await
      };
      tx.send(Message::Status(result.err())).ok();
      notify.notify_one();
    });
  }

  fn apply_optimistic(&mut self, command: &PlayerCommand) {
    let position = self.current_position();
    match command {
      PlayerCommand::Pause => {
        self.state.is_playing = false;
        self.state.progress = position;
      }
      PlayerCommand::Resume => self.state.is_playing = true,
      PlayerCommand::Stop => {
        self.state.is_playing = false;
        self.state.progress = Duration::ZERO;
      }
      PlayerCommand::Shuffle(state) => {
        self.state.shuffle = *state;
        self.state.queue.clear();
      }
      PlayerCommand::Repeat(state) => self.state.repeat = *state,
      PlayerCommand::Volume(volume) => self.state.volume = Some(*volume as u32),
      PlayerCommand::Seek(target) => self.state.progress = target.to_std().unwrap_or_default(),
      PlayerCommand::Next => {
        if self.state.queue.is_empty() {
          return;
        }
        let next = self.state.queue.remove(0);
        if let Some(current) = self.current_item() {
          self.history.push(current);
          if self.history.len() > 20 {
            self.history.remove(0);
          }
        }
        self.show_item(next);
        return;
      }
      PlayerCommand::Previous => {
        match (position > Duration::from_secs(3), self.history.pop()) {
          (false, Some(previous)) => {
            if let Some(current) = self.current_item() {
              self.state.queue.insert(0, current);
            }
            self.show_item(previous);
          }
          _ => {
            self.state.progress = Duration::ZERO;
            self.fetched_at = Instant::now();
            self.optimistic_until = Instant::now() + Duration::from_millis(1500);
            self.expect_uri = None;
          }
        }
        return;
      }
      PlayerCommand::PlayTrack(_, (shuffle, repeat)) | PlayerCommand::PlayPlaylist(_, (shuffle, repeat), ..) => {
        self.state.shuffle = *shuffle;
        self.state.repeat = *repeat;
        return;
      }
    }
    self.fetched_at = Instant::now();
    self.optimistic_until = Instant::now() + Duration::from_millis(1500);
    self.expect_uri = None;
  }

  fn current_position(&self) -> Duration {
    let mut position = self.state.progress;
    if self.state.is_playing {
      position += self.fetched_at.elapsed();
    }
    match self.state.length {
      Some(length) => position.min(length),
      None => position,
    }
  }

  fn perform_action(&mut self, action: Action) {
    let seek = |forwards: bool| {
      let position = self.current_position();
      let target = if forwards {
        position + Duration::from_secs(5)
      } else {
        position.saturating_sub(Duration::from_secs(5))
      };
      chrono::Duration::milliseconds(target.as_millis() as i64)
    };
    let volume = |diff: i32| (self.state.volume.unwrap_or(0) as i32 + diff).clamp(0, 100) as u8;

    let command = match action {
      Action::Quit => return,
      Action::Search => {
        self.focus = Focus::Search;
        return;
      }
      Action::PaneDown => return self.move_selection(1),
      Action::PaneUp => return self.move_selection(-1),
      Action::PaneTop => return self.move_selection(i32::MIN / 2),
      Action::PaneBottom => return self.move_selection(i32::MAX / 2),
      Action::PaneOpen => {
        return if self.tracks_view.is_some() { self.play_from_view() } else { self.activate_selected() }
      }
      Action::PaneTracks => return self.toggle_tracks_view(),
      Action::RemoveFromPlaylist => return self.remove_from_view(),
      Action::ToggleLike => return self.toggle_like(),
      Action::AddToQueue => return self.add_to_queue(),
      Action::AddToPlaylist => return self.open_playlist_picker(),
      Action::PaneParent => {
        if self.tracks_view.take().is_none() {
          self.pane.parent();
        }
        return;
      }
      Action::PaneShowHidden => {
        self.pane.show_hidden = !self.pane.show_hidden;
        return;
      }
      Action::PaneHide if self.tracks_view.is_none() => return self.toggle_hide(),
      Action::PanePin if self.tracks_view.is_none() => return self.toggle_pin(),
      Action::PaneHide | Action::PanePin => return,
      Action::PlayPause if self.state.is_playing => PlayerCommand::Pause,
      Action::PlayPause => PlayerCommand::Resume,
      Action::Stop => PlayerCommand::Stop,
      Action::Next => PlayerCommand::Next,
      Action::Previous => PlayerCommand::Previous,
      Action::ToggleShuffle => PlayerCommand::Shuffle(!self.state.shuffle),
      Action::CycleLoopStatus => PlayerCommand::Repeat(match self.state.repeat {
        RepeatState::Off => RepeatState::Context,
        RepeatState::Context => RepeatState::Track,
        RepeatState::Track => RepeatState::Off,
      }),
      Action::SeekBackwards => PlayerCommand::Seek(seek(false)),
      Action::SeekForwards => PlayerCommand::Seek(seek(true)),
      Action::IncreaseVolume => PlayerCommand::Volume(volume(10)),
      Action::DecreaseVolume => PlayerCommand::Volume(volume(-10)),
    };
    self.run(command);
  }

  fn refresh(&mut self, dirty: bool) {
    let size = termion::terminal_size().unwrap_or((80, 24));
    let resized = size != self.size;
    let animating = self.state.is_playing || self.levels.iter().any(|level| *level > 0.01);
    let viz_due = animating
      && matches!(self.menu, Menu::Closed)
      && self.last_tick.elapsed() >= Duration::from_millis(TICK_INTERVAL);
    if !dirty && !resized && !viz_due {
      return;
    }

    let mut out = String::from("\x1b[?2026h");
    if resized {
      self.size = size;
      if let Ok((width_px, height_px)) = termion::terminal_size_pixels() {
        if width_px > 0 && height_px > 0 {
          self.cell_px = (width_px as f32 / size.0 as f32, height_px as f32 / size.1 as f32);
        }
      }
      self.cell_aspect = (self.cell_px.1 / self.cell_px.0).clamp(1.0, 3.0);
      self.prev_rows.clear();
      self.placed = None;
      out.push_str("\x1b[2J");
      out.push_str(KITTY_CLEAR);
    }

    let layout = layout(size, &self.bindings);
    let player = layout.as_ref().and_then(|l| player_layout(l, self.cell_aspect, &self.settings));
    self.load_more_results();
    if let Some(layout) = &layout {
      self.pane.fit(layout.separator.saturating_sub(5));
      if let Some(view) = &mut self.tracks_view {
        view.fit(layout.separator.saturating_sub(5));
      }
    }
    let bars = player
      .as_ref()
      .map(|p| {
        let width = if self.settings.visualizer_width == 0 { p.w } else { self.settings.visualizer_width.min(p.w) };
        (width + 1) / (BAR_WIDTH + 1)
      })
      .unwrap_or(0);
    if viz_due || resized || (dirty && self.levels.len() != bars) {
      self.last_tick = Instant::now();
      if matches!(self.menu, Menu::Closed) {
        self.levels = self.visualizer.update(bars, self.state.is_playing).to_vec();
      }
    }
    if let Some(player) = &player {
      self.scale_cover(player);
    }

    let frame = compose(size, self);
    if frame.placement != self.placed {
      match &frame.placement {
        Some(placement) => {
          let cached = matches!(&self.kitty_cache, Some((uri, px, _)) if *uri == placement.uri && *px == placement.px);
          if !cached {
            if let Some(image) = &self.cover {
              self.kitty_cache = Some((placement.uri.clone(), placement.px, kitty_png_b64(image, placement.px)));
            }
          }
          if let Some((_, _, b64)) = &self.kitty_cache {
            write!(out, "{}", termion::cursor::Goto(placement.x as u16, placement.y as u16)).unwrap();
            out.push_str(&kitty_place(b64, placement.cols, placement.rows));
          }
        }
        None => out.push_str(KITTY_CLEAR),
      }
      self.placed = frame.placement.clone();
    }

    let mut rows = Vec::with_capacity(size.1 as usize);
    for y in 1..=size.1 as usize {
      let line = frame.grid.render_row(y);
      if self.prev_rows.get(y - 1) != Some(&line) {
        write!(out, "{}{}", termion::cursor::Goto(1, y as u16), line).unwrap();
      }
      rows.push(line);
    }
    self.prev_rows = rows;

    match frame.cursor {
      Some((x, y)) => write!(out, "{}{}", termion::cursor::Goto(x as u16, y as u16), termion::cursor::Show).unwrap(),
      None => write!(out, "{}", termion::cursor::Hide).unwrap(),
    }
    out.push_str("\x1b[?2026l");
    self.screen.write_all(out.as_bytes()).unwrap();
    self.screen.flush().unwrap();
  }

  fn scale_cover(&mut self, player: &PlayerLayout) {
    let Some((_, _, cols, rows)) = player.cover else {
      return;
    };
    let wanted = match self.graphics {
      Graphics::Kitty => return,
      Graphics::Plain => (cols, rows),
      _ => (cols, rows * 2),
    };
    if self.cover_scaled.as_ref().is_some_and(|(size, _)| *size == wanted) {
      return;
    }
    self.cover_scaled = self
      .cover
      .as_ref()
      .map(|image| (wanted, resize(image, wanted.0 as u32, wanted.1 as u32, FilterType::Triangle)));
  }
}

fn truncate(text: &str, width: usize) -> String {
  let mut used = 0;
  let mut out = String::new();
  for c in text.chars() {
    let char_width = c.width().unwrap_or(0);
    if used + char_width > width {
      break;
    }
    used += char_width;
    out.push(c);
  }
  out
}

fn pad(text: &str, width: usize) -> String {
  let text = truncate(text, width);
  let used = text.width();
  format!("{}{}", text, " ".repeat(width - used))
}

struct Layout {
  width: usize,
  height: usize,
  left: usize,
  separator: usize,
  hotkey_lines: Vec<String>,
}

fn layout(size: (u16, u16), bindings: &Bindings) -> Option<Layout> {
  let (width, height) = (size.0 as usize, size.1 as usize);
  let inner = width.saturating_sub(4).max(1);

  let mut items = vec!["esc menu".to_string()];
  items.extend(
    BAR_ACTIONS.iter().map(|action| format!("{} {}", bindings.display(*action), action.label())),
  );

  let mut hotkey_lines = vec![String::new()];
  for item in items {
    let line = hotkey_lines.last_mut().unwrap();
    if !line.is_empty() && line.chars().count() + 2 + item.chars().count() > inner {
      hotkey_lines.push(item);
    } else {
      if !line.is_empty() {
        line.push_str("  ");
      }
      line.push_str(&item);
    }
  }

  if width < 40 || height < hotkey_lines.len() + 8 {
    return None;
  }

  Some(Layout {
    width,
    height,
    left: (width / 4).clamp(16, 32),
    separator: height - hotkey_lines.len() - 1,
    hotkey_lines,
  })
}

#[derive(Clone, Copy, PartialEq)]
enum CellKind {
  Char(char),
  Wide,
  Image,
}

#[derive(Clone, Copy)]
struct Cell {
  kind: CellKind,
  style: u32,
}

struct Grid {
  width: usize,
  height: usize,
  cells: Vec<Cell>,
  styles: Vec<String>,
  ids: HashMap<String, u32>,
}

impl Grid {
  fn new(width: usize, height: usize) -> Grid {
    Grid {
      width,
      height,
      cells: vec![Cell { kind: CellKind::Char(' '), style: 0 }; width * height],
      styles: vec![String::new()],
      ids: HashMap::from([(String::new(), 0)]),
    }
  }

  fn intern(&mut self, style: &str) -> u32 {
    if let Some(id) = self.ids.get(style) {
      return *id;
    }
    let id = self.styles.len() as u32;
    self.styles.push(style.to_string());
    self.ids.insert(style.to_string(), id);
    id
  }

  fn put(&mut self, x: usize, y: usize, text: &str) {
    if y == 0 || y > self.height {
      return;
    }
    let mut x = x;
    let mut style = String::new();
    let mut id = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
      if c == '\x1b' {
        let mut sequence = String::from(c);
        while let Some(&next) = chars.peek() {
          sequence.push(next);
          chars.next();
          if next.is_ascii_alphabetic() {
            break;
          }
        }
        if sequence.ends_with('m') {
          if sequence == "\x1b[m" || sequence == "\x1b[0m" {
            style.clear();
          } else {
            style.push_str(&sequence);
          }
          id = self.intern(&style);
        }
        continue;
      }
      let width = c.width().unwrap_or(0);
      if width == 0 {
        continue;
      }
      if x >= 1 && x + width - 1 <= self.width {
        let index = (y - 1) * self.width + (x - 1);
        if x > 1 {
          if let CellKind::Char(previous) = self.cells[index - 1].kind {
            if previous.width() == Some(2) {
              self.cells[index - 1].kind = CellKind::Char(' ');
            }
          }
        }
        self.cells[index] = Cell { kind: CellKind::Char(c), style: id };
        if width == 2 {
          self.cells[index + 1] = Cell { kind: CellKind::Wide, style: id };
        }
      }
      x += width;
    }
  }

  fn overlay(&mut self, x: usize, y: usize, width: usize, suffix: &str) {
    if y == 0 || y > self.height || suffix.is_empty() {
      return;
    }
    for column in x..x + width {
      if column < 1 || column > self.width {
        continue;
      }
      let index = (y - 1) * self.width + (column - 1);
      if self.cells[index].kind == CellKind::Image {
        continue;
      }
      let style = format!("{}{}", self.styles[self.cells[index].style as usize], suffix);
      self.cells[index].style = self.intern(&style);
    }
  }

  fn fill_image(&mut self, x: usize, y: usize, cols: usize, rows: usize) {
    for row in y..y + rows {
      for col in x..x + cols {
        if row >= 1 && row <= self.height && col >= 1 && col <= self.width {
          self.cells[(row - 1) * self.width + (col - 1)] = Cell { kind: CellKind::Image, style: 0 };
        }
      }
    }
  }

  fn render_row(&self, y: usize) -> String {
    let mut out = String::new();
    let mut current = 0;
    let mut skipped = false;
    let mut previous_wide = false;
    for x in 0..self.width {
      let cell = self.cells[(y - 1) * self.width + x];
      let c = match cell.kind {
        CellKind::Image => {
          skipped = true;
          previous_wide = false;
          continue;
        }
        CellKind::Wide if previous_wide => {
          previous_wide = false;
          continue;
        }
        CellKind::Wide => ' ',
        CellKind::Char(c) => c,
      };
      if skipped {
        write!(out, "\x1b[{}G", x + 1).unwrap();
        skipped = false;
      }
      if cell.style != current {
        out.push_str("\x1b[m");
        out.push_str(&self.styles[cell.style as usize]);
        current = cell.style;
      }
      out.push(c);
      previous_wide = c.width() == Some(2);
    }
    out.push_str("\x1b[m");
    out
  }
}

#[derive(Clone, PartialEq)]
struct Placement {
  uri: String,
  x: usize,
  y: usize,
  cols: usize,
  rows: usize,
  px: u32,
}

struct Frame {
  grid: Grid,
  cursor: Option<(usize, usize)>,
  placement: Option<Placement>,
}

fn compose(size: (u16, u16), app: &App) -> Frame {
  let (pane, focus, bindings) = (&app.pane, app.focus, &app.bindings);
  let mut grid = Grid::new(size.0 as usize, size.1 as usize);

  let Some(layout) = layout(size, bindings) else {
    grid.put(1, 1, "terminal too small");
    return Frame { grid, cursor: None, placement: None };
  };
  let Layout { width, height, left, separator, .. } = layout;
  let divider = left + 2;
  let right = width - left - 3;

  grid.put(1, 1, &format!("┌{}┬{}┐", "─".repeat(left), "─".repeat(right)));
  for row in 2..separator {
    grid.put(1, row, "│");
    grid.put(divider, row, "│");
    grid.put(width, row, "│");
  }
  grid.put(1, 3, &format!("├{}┤", "─".repeat(left)));
  grid.put(1, separator, &format!("├{}┴{}┤", "─".repeat(left), "─".repeat(right)));
  for (i, line) in layout.hotkey_lines.iter().enumerate() {
    let row = separator + 1 + i;
    grid.put(1, row, "│");
    grid.put(width, row, "│");
    grid.put(2 + (width - 2 - line.width()) / 2, row, line);
  }
  grid.put(1, height, &format!("└{}┘", "─".repeat(width - 2)));

  let text_width = left - 2;
  let search_text = match (focus, app.query.is_empty()) {
    (Focus::Search, _) => search_tail(&app.query, text_width),
    (_, true) => format!("{}search bar{}", color::Fg(color::LightBlack), color::Fg(color::Reset)),
    (_, false) => truncate(&app.query, text_width),
  };
  grid.put(3, 2, &search_text);

  let header = if pane.results.is_some() {
    let back = bindings
      .keys
      .iter()
      .find(|(action, _)| *action == Action::PaneParent)
      .and_then(|(_, keys)| keys.iter().map(|key| key_to_string(*key)).min_by_key(|name| name.len()))
      .unwrap_or_default();
    format!("results  {} back", back)
  } else {
    "playlists".to_string()
  };
  let header_color = if focus == Focus::Playlists {
    format!("{}", color::Fg(color::Green))
  } else {
    format!("{}", color::Fg(color::Reset))
  };
  grid.put(3, 4, &format!("{}{}{}", header_color, truncate(&header, text_width), color::Fg(color::Reset)));

  let rows = separator.saturating_sub(5);
  for (i, entry) in pane.entries().iter().skip(pane.scroll).take(rows).enumerate() {
    let (label, entry_color) = match entry {
      Entry::Result(index) => {
        (pane.results.as_ref().map(|r| r[*index].0.clone()).unwrap_or_default(), String::new())
      }
      Entry::Playlist(index) => {
        let name = &pane.playlists[*index].0;
        if pane.hidden_playlists.contains(name) {
          (name.clone(), format!("{}", color::Fg(color::LightBlack)))
        } else if pane.pinned.contains(name) {
          (format!("★ {}", name), String::new())
        } else {
          (name.clone(), String::new())
        }
      }
    };
    let label = pad(&label, text_width);
    grid.put(3, 5 + i, &format!("{}{}{}{}", entry_color, label, termion::style::Reset, color::Fg(color::Reset)));
  }
  let head_style = if focus == Focus::Playlists { "\x1b[7m" } else { "\x1b[1m" };
  let entry_count = pane.entries().len().saturating_sub(pane.scroll).min(rows);
  let row_for = |index: usize| {
    index.checked_sub(pane.scroll).filter(|offset| *offset < entry_count).map(|offset| 5 + offset)
  };
  if let Some(row) = row_for(pane.cursor) {
    grid.overlay(3, row, text_width, head_style);
  }

  let placement = match &app.tracks_view {
    Some(view) => {
      draw_tracks_view(&mut grid, &layout, view, app);
      None
    }
    None => player_layout(&layout, app.cell_aspect, &app.settings).and_then(|player| draw_player(&mut grid, &player, app)),
  };
  if let (Some((text, _)), None) = (&app.notice, &app.status) {
    let row = player_layout(&layout, app.cell_aspect, &app.settings)
      .filter(|_| app.tracks_view.is_none())
      .map(|player| player.status_y)
      .unwrap_or(layout.separator.saturating_sub(1));
    let accent = app.graphics.fg(app.palette.accent);
    grid.put(layout.left + 4, row, &format!("{}{}\x1b[m", accent, truncate(text, layout.width.saturating_sub(layout.left + 7))));
  }
  draw_menu(&mut grid, size, app);

  let cursor = (app.focus == Focus::Search && matches!(app.menu, Menu::Closed))
    .then(|| (3 + search_tail(&app.query, text_width).width(), 2));
  Frame { grid, cursor, placement }
}

fn search_tail(query: &str, width: usize) -> String {
  let mut used = 0;
  let mut tail: Vec<char> = query
    .chars()
    .rev()
    .take_while(|c| {
      used += c.width().unwrap_or(0);
      used < width
    })
    .collect();
  tail.reverse();
  tail.into_iter().collect()
}

struct PlayerLayout {
  x: usize,
  w: usize,
  cover: Option<(usize, usize, usize, usize)>,
  info: (usize, usize, usize),
  queue: Option<(usize, usize, usize, usize)>,
  status_y: usize,
  timeline_y: usize,
  viz_y: usize,
  viz_h: usize,
}

fn player_layout(layout: &Layout, aspect: f32, settings: &Settings) -> Option<PlayerLayout> {
  let x = layout.left + 4;
  let right = layout.width.checked_sub(2)?;
  let y = 2;
  let bottom = layout.separator.checked_sub(1)?;
  if right < x + 20 || bottom < y + 5 {
    return None;
  }
  let (w, h) = (right - x + 1, bottom - y + 1);
  let viz_h = if settings.visualizer_height == 0 {
    (h / 3).clamp(2, 12)
  } else {
    settings.visualizer_height.min(h.saturating_sub(4)).max(2)
  };
  let top_h = h - viz_h - 2;

  let mut cover_rows = top_h.min(16);
  while cover_rows >= 4 && (cover_rows as f32 * aspect).round() as usize > w / 2 {
    cover_rows -= 1;
  }
  let cover = (settings.show_cover && cover_rows >= 4).then(|| (x, y, (cover_rows as f32 * aspect).round() as usize, cover_rows));
  let queue = (w >= 70 && settings.queue_length > 0).then(|| {
    let queue_w = (w / 3).min(36);
    (right - queue_w + 1, y, queue_w, top_h)
  });
  let info_x = cover.map(|(cx, _, cols, _)| cx + cols + 2).unwrap_or(x);
  let info_right = queue.map(|(qx, ..)| qx.saturating_sub(2)).unwrap_or(right);
  Some(PlayerLayout {
    x,
    w,
    cover,
    info: (info_x, y + 1, info_right.saturating_sub(info_x) + 1),
    queue,
    status_y: y + top_h,
    timeline_y: y + top_h + 1,
    viz_y: y + top_h + 2,
    viz_h,
  })
}

fn fmt_time(duration: Duration) -> String {
  let secs = duration.as_secs();
  format!("{}:{:02}", secs / 60, secs % 60)
}

fn timeline_bar(player: &PlayerLayout, length: Duration) -> (usize, usize) {
  let times_width = fmt_time(length).len() * 2 + 3;
  (player.x, player.w.saturating_sub(times_width + 1).max(5))
}

fn render_bars(levels: &[f32], palette: &Palette, height: usize, graphics: Graphics) -> Vec<String> {
  const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
  let h = height as i32;
  let count = levels.len();
  (0..h)
    .map(|row| {
      let mut line = String::new();
      for (i, level) in levels.iter().enumerate() {
        line.push_str(&graphics.fg(palette.level_color(*level)));
        let eighths = (level * h as f32 * 8.0) as i32;
        let fill = (eighths - (h - 1 - row) * 8).clamp(0, 8) as usize;
        let glyph = if fill == 0 && row == h - 1 { '▁' } else { BLOCKS[fill] };
        for _ in 0..BAR_WIDTH {
          line.push(glyph);
        }
        if i + 1 < count {
          line.push(' ');
        }
      }
      line.push_str("\x1b[m");
      line
    })
    .collect()
}

fn draw_tracks_view(grid: &mut Grid, layout: &Layout, view: &TracksView, app: &App) {
  const DIM: &str = "\x1b[2m";
  const BOLD: &str = "\x1b[1m";
  const RESET: &str = "\x1b[m";
  let x = layout.left + 4;
  let Some(right) = layout.width.checked_sub(2) else {
    return;
  };
  if right < x + 10 {
    return;
  }
  let width = right - x + 1;
  let accent = app.graphics.fg(app.palette.accent);
  let hint = match &view.tracks {
    Some(Ok(tracks)) => {
      let remove = if app.pane.editable.contains(&view.id.uri()) {
        format!("  {} remove", app.bindings.display(Action::RemoveFromPlaylist))
      } else {
        String::new()
      };
      format!("{} songs  enter play from here  {} close{}", tracks.len(), app.bindings.display(Action::PaneTracks), remove)
    }
    Some(Err(_)) => "o close".to_string(),
    None => "loading songs...".to_string(),
  };
  let name = truncate(&view.name, width);
  let room = width.saturating_sub(name.width() + 2);
  grid.put(x, 2, &format!("{BOLD}{}{}{RESET}  {DIM}{}{RESET}", accent, name, truncate(&hint, room)));
  match &view.tracks {
    Some(Ok(tracks)) => {
      let rows = layout.separator.saturating_sub(5);
      for (i, track) in tracks.iter().skip(view.scroll).take(rows).enumerate() {
        let index = view.scroll + i;
        let length = fmt_time(track.length);
        let title_width = width.saturating_sub(length.len() + 7);
        let line = format!("{:>3}. {} {}", index + 1, pad(&format!("{} - {}", track.artists, track.title), title_width), length);
        grid.put(x, 4 + i, &truncate(&line, width));
        if index == view.cursor {
          grid.overlay(x, 4 + i, width, "\x1b[7m");
        }
      }
    }
    Some(Err(message)) => grid.put(x, 4, &format!("{}{}{RESET}", color::Fg(color::Red), truncate(message, width))),
    None => {}
  }
}

fn draw_player(grid: &mut Grid, player: &PlayerLayout, app: &App) -> Option<Placement> {
  const DIM: &str = "\x1b[2m";
  const BOLD: &str = "\x1b[1m";
  const RESET: &str = "\x1b[m";
  let state = &app.state;
  let graphics = app.graphics;
  let accent = graphics.fg(app.palette.accent);
  let color_at = |index: usize| graphics.fg(app.palette.color(index));
  let mut placement = None;

  if let Some((x, y, cols, rows)) = player.cover {
    if graphics == Graphics::Kitty {
      if let (Some(image), true) = (&app.cover, matches!(app.menu, Menu::Closed)) {
        let px = ((rows as f32 * app.cell_px.1) as u32).min(image.width().max(image.height())).max(16);
        grid.fill_image(x, y, cols, rows);
        placement = Some(Placement { uri: state.uri.clone().unwrap_or_default(), x, y, cols, rows, px });
      }
    } else if let Some((_, image)) = &app.cover_scaled {
      const RAMP: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
      for row in 0..rows {
        let mut line = String::new();
        for col in 0..cols {
          if graphics == Graphics::Plain {
            let p = image.get_pixel(col as u32, row as u32);
            let luma = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
            line.push(RAMP[(luma as usize * RAMP.len() / 256).min(RAMP.len() - 1)]);
          } else {
            let top = image.get_pixel(col as u32, (row * 2) as u32);
            let bottom = image.get_pixel(col as u32, (row * 2 + 1) as u32);
            line.push_str(RESET);
            line.push_str(&graphics.fg((top[0], top[1], top[2])));
            line.push_str(&graphics.bg((bottom[0], bottom[1], bottom[2])));
            line.push('▀');
          }
        }
        grid.put(x, y + row, &line);
      }
    }
  }

  let (info_x, info_y, info_w) = player.info;
  if info_w >= 8 {
    match &state.title {
      Some(title) => {
        grid.put(info_x, info_y, &format!("{BOLD}{}{}{RESET}", accent, truncate(title, info_w)));
        grid.put(info_x, info_y + 1, &truncate(state.artists.as_deref().unwrap_or(""), info_w));
        let liked = state.uri.as_ref().is_some_and(|uri| app.liked.contains(uri));
        let heart = if liked { format!("{}♥", accent) } else { "\x1b[90m♥".to_string() };
        let album = state.album.as_deref().unwrap_or("");
        let album = truncate(album, info_w.saturating_sub(2));
        let heart_x = info_x + album.width() + if album.is_empty() { 0 } else { 1 };
        if !album.is_empty() {
          grid.put(info_x, info_y + 2, &format!("{DIM}{}{RESET}", album));
        }
        grid.put(heart_x, info_y + 2, &format!("{}{RESET}", heart));
        let status = if state.is_playing { "▶ playing" } else { "▮▮ paused" };
        grid.put(info_x, info_y + 4, &format!("{}{}{RESET}", color_at(1), truncate(status, info_w)));
        let volume = state.volume.map(|v| format!("vol {}%", v)).unwrap_or_else(|| "vol n/a".to_string());
        grid.put(info_x, info_y + 5, &truncate(&volume, info_w));
        let lit = |on: bool, text: &str, color: String| {
          if on {
            format!("{}{}{RESET}", color, truncate(text, info_w))
          } else {
            format!("{DIM}{}{RESET}", truncate(text, info_w))
          }
        };
        grid.put(info_x, info_y + 6, &lit(state.shuffle, if state.shuffle { "shuffle on" } else { "shuffle off" }, color_at(2)));
        let repeat = match state.repeat {
          RepeatState::Off => "repeat off",
          RepeatState::Context => "repeat all",
          RepeatState::Track => "repeat one",
        };
        grid.put(info_x, info_y + 7, &lit(state.repeat != RepeatState::Off, repeat, color_at(3)));
        if let Some(device) = &state.device_name {
          grid.put(info_x, info_y + 9, &format!("{DIM}{}{RESET}", truncate(device, info_w)));
        }
      }
      None => {
        grid.put(info_x, info_y, &truncate("select music to play", info_w));
      }
    }
  }

  if let (Some((x, y, w, h)), true) = (player.queue, state.title.is_some()) {
    grid.put(x, y, &format!("{BOLD}{}up next{RESET}", color_at(1)));
    for (i, entry) in state.queue.iter().take(h.saturating_sub(1).min(app.settings.queue_length)).enumerate() {
      grid.put(x, y + 1 + i, &truncate(&format!("{:>2}. {} - {}", i + 1, entry.artists, entry.title), w));
    }
  }

  if let Some(status) = &app.status {
    grid.put(player.x, player.status_y, &format!("{}{}{RESET}", color::Fg(color::Red), truncate(status, player.w)));
  }

  let position = app.current_position();
  let length = state.length.unwrap_or_default();
  let (bar_x, bar_w) = timeline_bar(player, length);
  let done = if length.is_zero() {
    0
  } else {
    ((bar_w as f64 * position.as_secs_f64() / length.as_secs_f64()) as usize).min(bar_w)
  };
  let filled: String = (0..done)
    .map(|i| format!("{}━", graphics.fg(app.palette.gradient(i as f32 / bar_w.max(1) as f32))))
    .collect::<String>()
    + RESET;
  let length_text = fmt_time(length);
  let times = format!("{:>w$} / {}", fmt_time(position.min(length)), length_text, w = length_text.len());
  grid.put(
    bar_x,
    player.timeline_y,
    &format!("{}{DIM}{}{RESET} {}", filled, "━".repeat(bar_w - done), times),
  );

  let bars = app.levels.len();
  let bars_width = (bars * (BAR_WIDTH + 1)).saturating_sub(1);
  let vis_x = player.x + player.w.saturating_sub(bars_width) / 2;
  for (i, line) in render_bars(&app.levels, &app.palette, player.viz_h, graphics).into_iter().enumerate() {
    grid.put(vis_x, player.viz_y + i, &line);
  }
  placement
}

fn draw_menu(grid: &mut Grid, size: (u16, u16), app: &App) {
  let (bindings, settings, menu) = (&app.bindings, &app.settings, &app.menu);
  let (width, height) = (size.0 as usize, size.1 as usize);
  let (title, rows, hint, cursor, capturing) = match menu {
    Menu::Closed => return,
    Menu::Main(cursor) => (
      "menu",
      MENU_ITEMS.iter().map(|item| (item.to_string(), String::new())).collect::<Vec<_>>(),
      "enter select  esc resume".into(),
      *cursor,
      false,
    ),
    Menu::AddToPlaylist { cursor, .. } => {
      let target = match menu {
        Menu::AddToPlaylist { uri, .. } => uri.as_str(),
        _ => "",
      };
      let rows: Vec<(String, String)> = app
        .editable_playlists()
        .iter()
        .map(|(name, id)| {
          let marker = match app.contents.get(&id.uri()) {
            Some(uris) if uris.contains(target) => "✓ in playlist",
            Some(_) => "",
            None => "…",
          };
          (name.clone(), marker.to_string())
        })
        .collect();
      ("add to playlist", rows, format!("enter add  esc cancel  {} hidden", bindings.display(Action::PaneShowHidden)).into(), *cursor, false)
    }
    Menu::Settings(cursor) => ("settings", settings.rows(), "j/k select  left/right change  esc back".into(), *cursor, false),
    Menu::Hotkeys { cursor, capturing } => {
      let mut rows: Vec<(String, String)> = sorted_actions()
        .iter()
        .map(|action| (action.label().to_string(), bindings.display(*action)))
        .collect();
      rows.push(("reset to defaults".to_string(), String::new()));
      let hint: std::borrow::Cow<str> = if *capturing { "press the new key  esc cancel".into() } else { "enter rebind  esc back".into() };
      ("hotkeys", rows, hint, *cursor, *capturing)
    }
  };

  let box_width = if matches!(menu, Menu::Main(_)) { 30 } else if matches!(menu, Menu::Settings(_) | Menu::AddToPlaylist { .. }) { 50 } else { 52 }.min(width.saturating_sub(2));
  let visible = rows.len().min(height.saturating_sub(5));
  let box_height = visible + 4;
  if box_width < 20 || visible == 0 {
    return;
  }
  let x = (width - box_width) / 2 + 1;
  let y = (height - box_height) / 2 + 1;
  let inner = box_width - 2;

  let title_text = format!("┤ {} ├", title);
  grid.put(x, y, &format!("┌{}{}┐", title_text, "─".repeat(inner.saturating_sub(title_text.chars().count()))));
  let start = cursor.saturating_sub(visible / 2).min(rows.len() - visible);
  for (i, (name, keys)) in rows.iter().skip(start).take(visible).enumerate() {
    let selected = start + i == cursor;
    let keys = if selected && capturing { "press a key...".to_string() } else { keys.clone() };
    let name_width = inner.saturating_sub(keys.chars().count() + 2);
    let line = format!(" {}{} ", pad(name, name_width), keys);
    grid.put(x, y + 1 + i, &format!("│{}│", line));
  }
  let menu_row = |index: usize| index.checked_sub(start).filter(|offset| *offset < visible).map(|offset| y + 1 + offset);
  if let Some(row) = menu_row(cursor) {
    grid.overlay(x + 1, row, inner, "\x1b[7m");
  }

  grid.put(x, y + visible + 1, &format!("│{:^w$}│", truncate(&hint, inner), w = inner));
  grid.put(x, y + visible + 2, &format!("└{}┘", "─".repeat(inner)));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  //connect to spotify
  let config_str = fs::read_to_string("config.toml")?;
  let config: ConfigFile = toml::from_str(&config_str)?;

  let creds = Credentials::new(&config.spotify.client_id, &config.spotify.client_secret);
  let oauth = OAuth {
    redirect_uri: config.spotify.redirect_uri.clone(),
    scopes: ["user-modify-playback-state", "user-read-playback-state", "playlist-read-private", "playlist-read-collaborative", "user-read-recently-played", "user-library-read", "user-library-modify", "playlist-modify-public", "playlist-modify-private"]
      .into_iter()
      .map(String::from)
      .collect(),
      ..OAuth::default()
  };

  let spotify_config = Config {
    token_cached: true,
    ..Config::default()
  };

  let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);
  let url = spotify.get_authorize_url(false)?;
  spotify.prompt_for_token(&url).await?;
  println!("connected");

  let (spotifyd, local_device_id) = restart_spotifyd(&spotify).await;
  let owns_spotifyd = spotifyd.is_some();
  let local_names = local_device_names();

  let screen = MouseTerminal::from(
    stdout()
      .into_raw_mode()
      .expect("Failed to initialize terminal raw mode")
      .into_alternate_screen()
      .expect("Failed to enter alternate screen"),
  );

  let (bindings, bindings_error) = Bindings::load();
  let (settings, settings_error) = Settings::load();
  let (tx, rx) = channel();
  let notify = Arc::new(Notify::new());
  let mut app = App {
    bindings,
    settings,
    local_names,
    local_device_id,
    menu: Menu::Closed,
    pane: PlaylistPane::new(PinsFile::default()),
    tracks_view: None,
    focus: Focus::Playlists,
    query: String::new(),
    search_id: 0,
    visualizer: Visualizer::start(),
    levels: Vec::new(),
    cover: None,
    cover_url: None,
    covers: HashMap::new(),
    covers_inflight: HashSet::new(),
    cover_scaled: None,
    graphics: Graphics::detect(),
    palette: Palette::default_palette(),
    cell_px: (8.0, 16.0),
    cell_aspect: 2.0,
    prev_rows: Vec::new(),
    placed: None,
    kitty_cache: None,
    last_tick: Instant::now(),
    contents: HashMap::new(),
    contents_inflight: HashSet::new(),
    pending_remove: None,
    liked: HashSet::new(),
    liked_checked: HashSet::new(),
    notice: None,
    optimistic_until: Instant::now(),
    expect_uri: None,
    history: Vec::new(),
    defaults_applied: false,
    events: termion::async_stdin().events(),
    screen,
    spotify,
    state: PlaybackState::new(),
    size: (0, 0),
    fetched_at: Instant::now(),
    status: bindings_error.or(settings_error),
    spotifyd,
    tx,
    rx,
    notify,
    refresh_queue: Arc::new(AtomicBool::new(false)),
    owns_spotifyd: Arc::new(AtomicBool::new(owns_spotifyd)),
    last_watch: Instant::now(),
    device_seen: false,
    device_missing: 0,
  };
  let pins = load_pins().unwrap_or_else(|message| {
    app.status = Some(message);
    PinsFile::default()
  });
  app.pane = PlaylistPane::new(pins);

  spawn_poller(app.spotify.clone(), app.tx.clone(), app.notify.clone(), app.refresh_queue.clone(), app.owns_spotifyd.clone());
  let (spotify, tx) = (app.spotify.clone(), app.tx.clone());
  tokio::spawn(async move {
    tx.send(Message::Playlists(fetch_playlists(&spotify).await)).ok();
  });
  app.main_loop();
  Ok(())
}
