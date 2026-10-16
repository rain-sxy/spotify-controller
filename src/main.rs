use std::borrow::Cow;
use std::io::{stdout, Stdout, Write};
use std::time::Duration;
use std::fs;

use serde::Deserialize;

use termion::color;
use termion::input::TermRead;
use termion::raw::{IntoRawMode, RawTerminal};
use termion::screen::{AlternateScreen, IntoAlternateScreen};

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
    }
  }

  fn is_enabled(&self, player: &Player) -> bool {
    match *self {
      Action::Quit => true,
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
}

//THIS ISNT USED ANYWHERE I CANT FIGURE OUT INPUT AND ITS 1AM PLEASE HELP
//IT WORKS AS A SEARCH FUNCTION IF I CAN PASS INPUT INTO IT BUT IM TOO TIRED
//FOR THIS RIGHT NOW :SOB:
//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
async fn search() -> Result<(), Box<dyn std::error::Error>> {
let mut query = String::new();
    loop {
        print!("\x1B[2J\x1B[1;1H");
        println!("Enter search:");

        let mut search_input = String::new();
        std::io::stdin().read_line(&mut search_input)?;
        query = search_input.trim().to_string();

        if !query.is_empty() {
            break;
        }

        println!("Must input search, press Enter to continue");
        let _ = std::io::stdin().read_line(&mut String::new());
    }

    let search_result_limit = 10;
    let mut search_result_offset = 0u32;

    loop {
        print!("\x1B[2J\x1B[1;1H");

        let search_results = spotify.search(
        &query,
        SearchType::Track,
        None,
        None,
        Some(search_result_limit),
        Some(search_result_offset)
    ).await?;

        let mut current_tracks = Vec::new();
        if let SearchResult::Tracks(results) = search_results {
            println!("found {} results for '{}':", results.total, query);

            if results.items.is_empty() {
                println!("No tracks found for '{}'", query);
            }
            current_tracks = results.items;
            
            for (i, track) in current_tracks.iter().enumerate() {
                let artists: Vec<String> = track.artists.iter().map(|a| a.name.clone()).collect();
                println!("{}. {} by {}", search_result_offset + i as u32 + 1, track.name, artists.join(", "));
            }

        }

        println!("\nType a track number to play it, or type 'next', 'prev', 'search', or 'quit':");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let command = input.trim().to_lowercase();

        if let Ok(track_choice) = command.parse::<u32>() {
            let start_index = search_result_offset + 1;
            let end_index = search_result_offset + current_tracks.len() as u32;

            if track_choice >= start_index && track_choice <= end_index {
                let local_index = (track_choice - start_index) as usize;
                if let Some(track) = current_tracks.get(local_index) {
                    println!("Playing: {} by {}", track.name, track.artists[0].name);

                    if let Some(track_id) = &track.id {
                        spotify.start_uris_playback(
                            [rspotify::model::PlayableId::Track(track_id.clone())],
                            None,
                            None,
                            None,
                        ).await?;
                    } else {
                        println!("Selected track has no valid ID");
                    }
                }
            } else {
                println!("Selected track out of range");
            }
        } else {
            match command.as_str() {
                "next" => {
                    search_result_offset += search_result_limit;
                }
                "prev" | "previous" => {
                    if search_result_offset >= search_result_limit {
                        search_result_offset -= search_result_limit;
                    } else {
                        println!("You are already on the first page");
                    }
                }
                "search" => {
                    let new_query;
                    loop{
                        println!("Enter new search:");
                        let mut new_search_input = String::new();
                        std::io::stdin().read_line(&mut new_search_input)?;
                        let trimmed_search_input = new_search_input.trim().to_string();

                        if !trimmed_search_input.is_empty() {
                            new_query = trimmed_search_input;
                            break;
                        }
                        println!("Must input search");
                    }
                    query = new_query;
                    search_result_offset = 0;
                }
                "quit" | "exit" => {
                    println!("Quitting controller");
                    break;
                }
                _ => {
                    println!("Unknown command");
                }
            }
        }
    }
    Ok(())
}
//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA

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
    scopes: ["user-modify-playback-state", "user-read-playback-state"]
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
  let player = PlayerFinder::new()
    .unwrap()
    .find_all()
    .expect("Could not find any player")
    .into_iter()
    .find(|p| {
        p.identity().to_lowercase().contains("spotifyd")
    })
  .expect("Could not find spotifyd daemon");
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
