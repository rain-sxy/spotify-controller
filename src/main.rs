use mpris::{
    LoopStatus, Metadata, PlaybackStatus, Player, PlayerFinder, Progress, ProgressTick,
    ProgressTracker, TrackID, TrackList,
};


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
  fn from_key(key: termion::event:Key) -> Option<Action> {
    use crate::Action::*;
    use termion::event::Key;

    match key {
      Key::Ctrl('c') | Key::Esc | Key::Char('q') => Some(Quit),

    }
  }
}





// Pauses currently playing media and prints metadata information about that
// media.
// If no player is running, exits with an error.
fn main() {
  //find active mpris player
  let player = PlayerFinder::new()
    .expect("Could not connect to D-Bus")
    .find_active()
    .expect("Could not find any player");
  
  //attempt to pause mpris player
  player.pause().expect("Could not pause");
  
  //print metadata if it can be gotten
  let metadata = player.get_metadata().expect("Could not get metadata for player");
  println!("{:#?}", metadata);
}
