use std::borrow::Cow;
use std::io::{stdout, Stdout, Write};
use std::time::Duration;
use std::fs;

use serde::Deserialize;

use termion::color;
use termion::input::TermRead;
use termion::raw::{IntoRawMode, RawTerminal};
use termion::screen::{AlternateScreen, IntoAlternateScreen};

use futures::StreamExt;

use mpris::{
  LoopStatus, Metadata, PlaybackStatus, Player, PlayerFinder, Progress, ProgressTick,
  ProgressTracker, TrackID, TrackList,
};

use rspotify::prelude::*;
use rspotify::{AuthCodeSpotify, Credentials, OAuth, Config};
use rspotify::model::{SearchType, SearchResult};

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

#[derive(Clone, Copy)]
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
  Playlists,
}

//uhh ion remember why i changed this variable,
//i was doing so much random stuff to get inputs working 😭
type Screen = AlternateScreen<RawTerminal<Stdout>>;
const REFRESH_INTERVAL: u32 = 100; //100 msec

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
  Action::Playlists,
];

impl Action {
  fn from_key(key: termion::event::Key) -> Option<Action> {
    use crate::Action::*;
    use termion::event::Key;

    match key {
      Key::Ctrl('c') | Key::Esc | Key::Char('q') => Some(Quit),
      Key::Char(' ') => Some(PlayPause),
      Key::Char('s') => Some(Stop),
      Key::Char('n') => Some(Next),
      Key::Char('p') => Some(Previous),
      Key::Char('z') => Some(ToggleShuffle),
      Key::Char('x') => Some(CycleLoopStatus),
      Key::Char('+') => Some(IncreaseVolume),
      Key::Char('-') => Some(DecreaseVolume),
      Key::Right => Some(SeekForwards),
      Key::Left => Some(SeekBackwards),
      Key::Char('/') => Some(Search),
      Key::Char('l') => Some(Playlists),
      _ => None,
    }
  }

  fn key_name(&self) -> &'static str {
    match *self {
      Action::Quit => "q",
      Action::PlayPause => "Space",
      Action::Stop => "s",
      Action::Next => "n",
      Action::Previous => "p",
      Action::ToggleShuffle => "z",
      Action::CycleLoopStatus => "x",
      Action::SeekForwards => "Left",
      Action::SeekBackwards => "Right",
      Action::IncreaseVolume => "+",
      Action::DecreaseVolume => "-",
      Action::Search => "/",
      Action::Playlists => "l",
    }
  }

  fn description(&self) -> &'static str {
    match *self {
      Action::Quit => "Quit",
      Action::PlayPause => "Toggle play/pause",
      Action::Stop => "Stop",
      Action::Next => "Next media",
      Action::Previous => "Previous media",
      Action::ToggleShuffle => "Toggle shuffle",
      Action::CycleLoopStatus => "Cycle loop status",
      Action::SeekForwards => "Seek 5s forward",
      Action::SeekBackwards => "Seek 5s backward",
      Action::IncreaseVolume => "Increase volume",
      Action::DecreaseVolume => "Decrease volume",
      Action::Search => "Search",
      Action::Playlists => "View Playlists",
    }
  }

  fn is_enabled(&self, player: &Player) -> bool {
    match *self {
      Action::Quit | Action::Search | Action::Playlists => true,
      Action::PlayPause => player.can_pause().unwrap_or(false),
      Action::Stop => player.can_stop().unwrap_or(false),
      Action::Next => player.can_go_next().unwrap_or(false),
      Action::Previous => player.can_go_previous().unwrap_or(false),
      Action::ToggleShuffle | Action::CycleLoopStatus => {
        player.can_control().unwrap_or(false)
      }
      Action::IncreaseVolume => player
        .can_control()
        .and_then(|_| player.get_volume())
        .map(|vol| vol < 1.0)
        .unwrap_or(false),
      Action::DecreaseVolume => player
        .can_control()
        .and_then(|_| player.get_volume())
        .map(|vol| vol > 0.0)
        .unwrap_or(false),
      Action::SeekForwards | Action::SeekBackwards => player.can_seek().unwrap_or(false),
    }
  }

  fn should_exit(&self) -> bool {
    matches!(self, Action::Quit)
  }
}

struct App<'a> {
  player: &'a Player,
  progress_tracker: ProgressTracker<'a>,
  stdin: termion::AsyncReader,
  screen: Screen,
  spotify: AuthCodeSpotify,
}

impl<'a> App<'a> {
  fn main_loop(&mut self) {
    let mut should_continue = true;
    let mut should_refresh = true;

    while should_continue {
      while let Some(action) = self.next_action() {
        self.perform_action(action);
        if action.should_exit() {
          should_continue = false;
        }
        should_refresh = true;
      }
      self.tick_progress_and_refresh(should_refresh);
      should_refresh = false;
    }
  }

  fn next_action(&mut self) -> Option<Action> {
    (&mut self.stdin)
      .keys()
      .next()
      .and_then(|result| result.ok())
      .and_then(Action::from_key)
  }

  fn perform_action(&mut self, action: Action) {
    match action {
      Action::Quit => (),
      Action::PlayPause => control_player(self.player.play_pause()),
      Action::Stop => control_player(self.player.stop()),
      Action::Next => control_player(self.player.next()),
      Action::Previous => control_player(self.player.previous()),
      Action::ToggleShuffle => control_player(toggle_shuffle(self.player)),
      Action::CycleLoopStatus => control_player(cycle_loop_status(self.player)),
      Action::SeekBackwards => control_player(self.player.seek_backwards(&Duration::new(5, 0))),
      Action::SeekForwards => control_player(self.player.seek_forwards(&Duration::new(5, 0))),
      Action::IncreaseVolume => control_player(change_volume(self.player, 0.1)),
      Action::DecreaseVolume => control_player(change_volume(self.player, -0.1)),
      Action::Playlists => self.show_playlists(),
      Action::Search => {
        if let Some(search_query) = prompt_for_search(&mut self.screen, &mut self.stdin) {
          self.search_for(search_query);
        }
      }
    };
  }

  fn tick_progress_and_refresh(&mut self, should_refresh: bool) {
    let supports_position = self.supports_position();
    let ProgressTick {
      progress,
      progress_changed,
      track_list,
      track_list_changed,
      ..
    } = self.progress_tracker.tick();

    //trying to make progress bar work with seeking its kinda weird aaaaaaaaa (dont judge pls D:)
    let current_position = if supports_position {
        self.player.get_position().unwrap_or_else(|_| progress.position())
    } else {
        progress.position()
    };

    //dirty tracking to keep cpu usage lower, doesnt do anything if nothing has changed and doesnt refresh
    //progress bar if player does not support it
    if progress_changed || track_list_changed || should_refresh {
      let current_track_id = progress.metadata().track_id();
      clear_screen(&mut self.screen);
      print_instructions(&mut self.screen, self.player);
      print_playback_info(&mut self.screen, progress);
      if let Some(tracks) = track_list {
        let next_track = find_next_track(current_track_id, tracks, self.player);
        print_track_list(&mut self.screen, tracks, next_track);
      }
      print_progress_bar(&mut self.screen, progress, current_position, supports_position);
    } else if supports_position {
      clear_progress_bar(&mut self.screen);
      print_progress_bar(&mut self.screen, progress, current_position, supports_position);
    }
    self.screen.flush().unwrap();
  }

  fn supports_position(&self) -> bool {
    self.player.identity() != "Spotify"
  }

  //search for tracks using query
  fn search_for(&mut self, query: String) {
    let spotify = self.spotify.clone();

    let search_result = tokio::task::block_in_place(|| {
      tokio::runtime::Handle::current().block_on(async move {
        spotify.search(
          &query,
          SearchType::Track,
          None,
          None,
          Some(10),
          None,
        ).await
      })
    });

    let tracks = match search_result {
      Ok(SearchResult::Tracks(results)) => results.items,
      Ok(_) => Vec::new(),
      Err(e) => {
        eprintln!("Search failed: {}", e);
        Vec::new()
      }
    };

    if tracks.is_empty() {
      write!(self.screen, "\r\nNo tracks found\r\n").unwrap();
      self.screen.flush().ok();
      return;
    }

    for (i, track) in tracks.iter().enumerate() {
      let artists: Vec<String> = track.artists.iter().map(|a| a.name.clone()).collect();
      println!("\r\n {:2}. {} - {}\r\n", i + 1, artists.join(", "), track.name);
    }


    //track selection, this could totally be merged in different function with the
    //other search stuff but i really cant be asked
    use termion::event::Key;
    write!(self.screen, "\r\nEnter track number to play (1-10) or Esc to cancel: ").unwrap();
    self.screen.flush().ok();

    let mut input_str = String::new();
    loop {
      if let Some(Ok(key)) = (&mut self.stdin).keys().next() {
        match key {
          Key::Char('\n') => break,
          Key::Char(ch) => {
            if ch.is_ascii_digit() {
              input_str.push(ch);
              write!(self.screen, "{}", ch).unwrap();
              self.screen.flush().ok();
            }
          }
          Key::Backspace => {
            if !input_str.is_empty() {
              input_str.pop();
              write!(self.screen, "\x08 \x08").unwrap();
              self.screen.flush().ok();
            }
          }
          Key::Esc => return,
          _ => {}
        }
      } else {
        std::thread::sleep(Duration::from_millis(10));
      }
    }
    //play track number that was picked
    if let Ok(index) = input_str.parse::<usize>() {
      if index >= 1 && index <= tracks.len().min(10) {
        let chosen_track = &tracks[index - 1];
        if let Some(track_id) = chosen_track.id.clone() {
          let spotify_client = self.spotify.clone();

          let play_track = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
              spotify_client.start_uris_playback(
                [rspotify::model::PlayableId::Track(track_id.clone())],
                None,
                None,
                None,
              ).await
            })
          });
        }
      }
    }
  }

  fn show_playlists(&mut self) {
    let spotify = self.spotify.clone();

    let playlists_result = tokio::task::block_in_place(|| {
      tokio::runtime::Handle::current().block_on(async move {
        spotify.current_user_playlists().collect::<Vec<_>>().await
      })
    });
    let playlists: Vec<_> = match playlists_result.into_iter().collect::<Result<Vec<_>, _>>() {
      Ok(results) => results,
      Err(e) => {
        write!(self.screen, "Failed to fetch playlists: {}", e).ok();
        return;
      }
    };

    if playlists.is_empty() {
        write!(self.screen, "No playlists found").unwrap();
    }

    for (i, playlist) in playlists.iter().take(10).enumerate() {
      write!(self.screen, "\r\n {:2}. {}\r\n", i + 1, playlist.name).ok();
    }
    self.screen.flush().ok();

    use termion::event::Key;
    write!(self.screen, "\r\nEnter track number to play (1-10) or Esc to cancel: ").unwrap();
    self.screen.flush().ok();

    let mut input_str = String::new();
    loop {
      if let Some(Ok(key)) = (&mut self.stdin).keys().next() {
        match key {
          Key::Char('\n') => break,
          Key::Char(ch) => {
            if ch.is_ascii_digit() {
              input_str.push(ch);
              write!(self.screen, "{}", ch).unwrap();
              self.screen.flush().ok();
            }
          }
          Key::Backspace => {
            if !input_str.is_empty() {
              input_str.pop();
              write!(self.screen, "\x08 \x08").unwrap();
              self.screen.flush().ok();
            }
          }
          Key::Esc => return,
          _ => {}
        }
      } else {
        std::thread::sleep(Duration::from_millis(10));
      }
    }

    //play selected playlist
    //does not work with shuffle aaaaa
    if let Ok(index) = input_str.parse::<usize>() {
      if index >= 1 && index <= playlists.len().min(10) {
        let chosen_playlist = &playlists[index - 1];
        let playlist_id = chosen_playlist.id.clone();
        let context_id = PlayContextId::Playlist(playlist_id);
        let spotify_client = self.spotify.clone();

        let play_playlist = tokio::task::block_in_place(|| {
          tokio::runtime::Handle::current().block_on(async move {
            spotify_client.start_context_playback(
              context_id,
              None,
              None,
              None,
            ).await
          })
        });
      }
    }
  }
}

fn prompt_for_search(screen: &mut Screen, stdin: &mut termion::AsyncReader) -> Option<String> {
  use termion::event::Key;
  write!(screen, "\r\nSearch: ").unwrap();
  screen.flush().ok()?;

  let mut query = String::new();
  loop {
    if let Some(Ok(key)) = stdin.keys().next() {
      match key {
        Key::Char('\n') => break,
        Key::Char(ch) => {
        query.push(ch);
        write!(screen, "{}", ch).unwrap();
        screen.flush().ok()?;
      }
      Key::Backspace => {
        if !query.is_empty() {
          query.pop();
          write!(screen, "\x08 \x08").unwrap();
          screen.flush().ok()?;
        }
      }
        Key::Esc => return None,
        _ => {}
      }
    } else {
      std::thread::sleep(Duration::from_millis(10));
    }
  }
  Some(query)
}

fn find_next_track(
  current_track_id: Option<TrackID>,
  track_list: &TrackList,
  player: &Player,
  ) -> Option<Metadata> {
  if let Some(current_id) = current_track_id {
    track_list
      .metadata_iter(player)
      .ok()?
      .skip_while(|track| match track.track_id() {
        Some(id) => id != current_id,
        None => false,
      })
    .nth(1)
  } else {
    None
  }
}

fn control_player(result: Result<(), mpris::DBusError>) {
  result.expect("Could not control player");
}

fn toggle_shuffle(player: &Player) -> Result<(), mpris::DBusError> {
  player.set_shuffle(!player.get_shuffle()?)
}

fn cycle_loop_status(player: &Player) -> Result<(), mpris::DBusError> {
  let current_status = player.get_loop_status()?;
  let next_status = match current_status {
    LoopStatus::None => LoopStatus::Playlist,
    LoopStatus::Playlist => LoopStatus::Track,
    LoopStatus::Track => LoopStatus::None,
  };
  player.set_loop_status(next_status)
}

fn change_volume(player: &Player, diff: f64) -> Result<(), mpris::DBusError> {
  let current_volume = player.get_volume()?;
  let new_volume = (current_volume + diff).max(0.0).min(1.0);
  player.set_volume(new_volume)
}

fn find_spotifyd_player() -> Player {
  PlayerFinder::new()
    .unwrap()
    .find_all()
    .expect("Could not find any player")
    .into_iter()
    .find(|p| p.identity().to_lowercase().contains("spotifyd"))
    .expect("Could not find spotifyd daemon")
}

//////////////////////////////
//UI stuff!!! do whatever :p//
//////////////////////////////

//prints Instructions for controlling <media player>
//then key>>description from key_name and description functions
fn print_instructions(screen: &mut Screen, player: &Player) {
  let bold = termion::style::Bold;
  let nobold = termion::style::Reset;

  write!(
    screen,
    "{}Instructions for controlling {}:{}\r\n",
    bold,
    player.identity(),
    nobold,
    )
    .unwrap();

  for action in ACTIONS {
    let is_enabled = action.is_enabled(player);

    if is_enabled {
      write!(screen, "{}", color::Fg(color::Reset)).unwrap();
    } else {
      write!(screen, "{}", color::Fg(color::LightBlack)).unwrap();
    };

    write!(
      screen,
      "  {bold}{key:>5}{nobold} - {description}",
      bold = bold,
      nobold = nobold,
      key = action.key_name(),
      description = action.description(),
      )
      .unwrap();

    write!(
      screen,
      "{nostatecolor}\r\n",
      nostatecolor = color::Fg(color::Reset),
      )
      .unwrap();
  }
  write!(screen, "\r\n").unwrap();
}

fn print_playback_info(screen: &mut Screen, progress: &Progress) {
  let playback_string = match progress.playback_status() {
    PlaybackStatus::Playing => format!("{}▶", color::Fg(color::Green)),
    PlaybackStatus::Paused => format!("{}▮▮", color::Fg(color::LightBlack)),
    PlaybackStatus::Stopped => format!("{}◼", color::Fg(color::Red)),
  };

  let shuffle_string = if progress.shuffle() {
    format!("{}⮭⮯", color::Fg(color::Green))
  } else {
    format!("{}🠯🠯", color::Fg(color::LightBlack))
  };

  let loop_string = match progress.loop_status() {
    LoopStatus::None => format!("{}🠮", color::Fg(color::LightBlack)),
    LoopStatus::Playlist => format!("{}🔁", color::Fg(color::Green)),
    LoopStatus::Track => format!("{}🔂", color::Fg(color::Yellow)),
  };

  let volume_string = format!("(vol: {:3.0}%)", progress.current_volume() * 100.0);

  write!(
    screen,
    "{playback} {shuffle} {loop} {color_reset} ",
    playback = playback_string,
    shuffle = shuffle_string,
    loop = loop_string,
    color_reset = color::Fg(color::Reset),
    )
    .unwrap();
  print_track_info(screen, progress.metadata());
  write!(screen, " {volume}\r\n", volume = volume_string).unwrap();
}

fn print_track_info(screen: &mut Screen, track: &Metadata) {
  let artist_string: Cow<'_, str> = track
    .artists()
    .map(|artists| Cow::Owned(artists.join(" + ")))
    .unwrap_or_else(|| Cow::Borrowed("Unknown artist"));

    let title_string = track.title().unwrap_or("Unkown title");

  write!(
    screen,
    "{blue}{bold}{artist}{reset}{blue} - {title}{color_reset}",
    blue = color::Fg(color::Blue),
    color_reset = color::Fg(color::Reset),
    bold = termion::style::Bold,
    reset = termion::style::Reset,
    artist = artist_string,
    title = title_string,
    )
    .unwrap();
}

fn print_track_list(screen: &mut Screen, track_list: &TrackList, next_track: Option<Metadata>) {
  if let Some(track) = next_track {
    write!(
      screen,
      "{bold}Next:{reset} ",
      bold = termion::style::Bold,
      reset = termion::style::Reset,
      )
      .unwrap();
      print_track_info(screen, &track);
      write!(screen, ", ").unwrap();
  }
}

fn print_progress_bar(screen: &mut Screen, progress: &Progress, position: Duration, supports_position: bool) {
  let position_string: Cow<'_, str> = if supports_position {
    Cow::Owned(format_duration(position))
  } else {
    Cow::Borrowed("??:??:??")
  };

  let length_string: Cow<'_, str> = progress
    .length()
    .map(|s| Cow::Owned(format_duration(s)))
    .unwrap_or_else(|| Cow::Borrowed("??:??:??"));

  write!(
    screen,
    "{position} / {length}\r\n",
    position = position_string,
    length = length_string,
    )
    .unwrap();
}

fn clear_progress_bar(screen: &mut Screen) {
  write!(
    screen,
    "{}\r{}",
    termion::cursor::Up(1),
    termion::clear::CurrentLine,
    )
    .unwrap();
}

fn clear_screen(screen: &mut Screen) {
  write!(
    screen,
    "{}{}",
    termion::clear::All,
    termion::cursor::Goto(1, 1)
    )
    .unwrap();
}

fn format_duration(duration: Duration) -> String {
  let secs = duration.as_secs();
  let whole_hours = secs / (60 * 60);
  
  let secs = secs - whole_hours * 60 * 60;
  let whole_minutes = secs / 60;

  let secs = secs - whole_minutes * 60;

  format!("{:02}:{:02}:{:02}", whole_hours, whole_minutes, secs)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  //connect to spotify
  let config_str = fs::read_to_string("config.toml")?;
  let config: ConfigFile = toml::from_str(&config_str)?;

  let creds = Credentials::new(&config.spotify.client_id, &config.spotify.client_secret);
  let oauth = OAuth {
    redirect_uri: config.spotify.redirect_uri.clone(),
    scopes: ["user-modify-playback-state", "user-read-playback-state", "playlist-read-private", "playlist-read-collaborative"]
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
  println!("Connected");

  //look for spotifyd
  let player = find_spotifyd_player();

  let progress_tracker = player
    .track_progress(REFRESH_INTERVAL)
    .expect("Could not determine progress of player");

  let screen = stdout()
    .into_raw_mode()
    .expect("Failed to initialize terminal raw mode")
    .into_alternate_screen()
    .expect("Failed to enter alternate screen");

  let mut app = App {
    player: &player,
    progress_tracker,
    screen,
    stdin: termion::async_stdin(),
    spotify,
  };
  app.main_loop();
  Ok(())
}
