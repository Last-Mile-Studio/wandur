//! A Legends of the Jedi-like flood (`--page lotj` with `--rate`, and the shell bench's LotJ
//! scenario): the generated text with a room every few lines, each one a GMCP `Room.Info` with
//! a vnum, a planet and O/C exit states but no description, and the room text LOTJ shows (a
//! coloured title with its bracketed flags, the description, `Obvious exits:` and one line per
//! exit, occupants, an unterminated HP prompt). Rooms alternate between `Room.Info` before the
//! text and after it, so the client's description fill sees both orders.

const IAC: u8 = 255;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const GMCP: u8 = 201;

/// Generated lines between two rooms.
pub const LINES_PER_ROOM: usize = 20;
/// Rooms in the loop walked.
const ROOMS: u32 = 200;

/// The offer that starts GMCP.
pub fn offer() -> &'static [u8] {
    &[IAC, WILL, GMCP]
}

/// Room `step` of the walk, as LOTJ sends it.
pub fn room(step: u32) -> Vec<u8> {
    let vnum = 1000 + step % ROOMS;
    let name = format!("TRAINING: Hall {}", step % ROOMS);
    let flags = if step.is_multiple_of(3) { " [Bacta]" } else { "" };
    let info =
        format!(r#"Room.Info {{"vnum":{vnum},"name":"{name}","planet":"Academy","exits":{{"east":"O","west":"C"}}}}"#);
    let mut gmcp = vec![IAC, SB, GMCP];
    gmcp.extend_from_slice(info.as_bytes());
    gmcp.extend_from_slice(&[IAC, SE]);
    let text = format!(
        "\r\n\x1b[1;36m{name}{flags}\x1b[0m\r\n\
         \x1b[0;37mDurasteel panels line hall {n} of the academy, scuffed by years of boots.\x1b[0m\r\n\
         \x1b[0;37mA holoprojector flickers above a bench near the eastern door.\x1b[0m\r\n\
         \x1b[1;37mObvious exits:\x1b[0m\r\n\
         East  - TRAINING: Hall {e}\r\n\
         West  - TRAINING: Hall {w}\r\n\
         \x1b[1;33mA training droid is standing here.\x1b[0m\r\n\
         \x1b[0;35mA datapad lies here.\x1b[0m\r\n\
         \r\n*((==HP==((|||| ",
        n = step % ROOMS,
        e = (step + 1) % ROOMS,
        w = (step + ROOMS - 1) % ROOMS,
    );
    let mut out = Vec::with_capacity(gmcp.len() + text.len());
    if step.is_multiple_of(2) {
        out.extend(gmcp);
        out.extend_from_slice(text.as_bytes());
    } else {
        out.extend_from_slice(text.as_bytes());
        out.extend(gmcp);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use wandur_core::map::{MapSession, OptionState, WalkGate};
    use wandur_core::protocol::parse_gmcp;

    #[test]
    fn rooms_fill_their_descriptions_in_both_orders() {
        let mut map = MapSession::new();
        map.set_gmcp(OptionState::Enabled);
        let gate = WalkGate {
            connected: true,
            ..WalkGate::default()
        };
        for step in 0..6 {
            let bytes = super::room(step);
            let start = bytes.windows(3).position(|w| w == [255, 250, 201]).unwrap();
            let end = bytes.windows(2).position(|w| w == [255, 240]).unwrap();
            let text_before = String::from_utf8_lossy(&bytes[..start]).into_owned();
            let text_after = String::from_utf8_lossy(&bytes[end + 2..]).into_owned();
            let now = Instant::now();
            map.track_output(&text_before, gate, now);
            let room = wandur_core::map::decode::from_gmcp(&parse_gmcp(&bytes[start + 3..end]).unwrap()).unwrap();
            map.observe_room(room, gate, now);
            map.track_output(&text_after, gate, now);
        }
        let filled = map
            .tracker()
            .rooms()
            .filter(|r| r.description.starts_with("Durasteel panels"))
            .count();
        assert_eq!(filled, 6);
        assert!(map.tracker().rooms().all(|r| !r.description.contains("droid")));
    }
}
