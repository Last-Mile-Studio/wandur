//! The offline demo world: five scripted rooms that run on this machine, so a person can try the
//! client without a server. The rooms, their text and the commands are the C# client's
//! `DemoSession`; the banner uses the current name of the app.
//!
//! The world is a plain state machine: [`DemoWorld::start`] returns the opening text and
//! [`DemoWorld::command`] the reply to one command, both as terminal text with ANSI colours and
//! `\n` line ends, each ending at the `> ` prompt.

/// One room: its name, its description and its exits (direction, room index) in display order.
struct Room {
    name: &'static str,
    description: &'static str,
    exits: &'static [(&'static str, usize)],
}

const ROOMS: [Room; 5] = [
    Room {
        name: "The Lantern & the Rain",
        description: "Rain taps against the leaded windows of a small wayside inn.\n\
An amber lantern hangs above the hearth, catching the edges of\n\
a map spread across an oak table. Somewhere, a kettle sings.\n\
\n\
The innkeeper looks up. \"There's still a little daylight left.\n\
The old market is north of here, if you've a mind to wander.\"",
        exits: &[("north", 1), ("east", 2)],
    },
    Room {
        name: "The Old Market",
        description: "Water gathers between worn cobblestones. The market stalls\n\
are shuttered for the evening, but a flower seller has left\n\
a jar of white asters beneath the clock tower.\n\
\n\
A narrow stair curls upward into the tower.",
        exits: &[("south", 0), ("up", 3)],
    },
    Room {
        name: "A Bridge of Moss & Stone",
        description: "A low stone bridge crosses a stream the color of old silver.\n\
Beyond the arch, the forest breathes mist into the valley.\n\
The lights of the inn glow through the rain to the west.",
        exits: &[("west", 0), ("east", 4)],
    },
    Room {
        name: "Above the Rooftops",
        description: "The clock has stopped at a quarter past something. From here\n\
you can see the inn, the bridge, and the dark line of the woods.\n\
A swallow has made a home inside the silent bell.",
        exits: &[("down", 1)],
    },
    Room {
        name: "The Edge of the Wood",
        description: "Wet pine needles soften your footsteps. Fireflies gather\n\
between the roots of an ancient oak. A weathered sign reads:\n\
\"Every world begins with a first step.\"",
        exits: &[("west", 2)],
    },
];

/// The status line while the demo runs (the C# text).
pub const STATUS: &str = "Offline demo · no server required";
/// The status line after it ends.
pub const ENDED: &str = "Demo ended";

const BANNER: &str = "\x1b[38;2;209;155;104m  W A N D U R\x1b[0m\n  A little world, waiting to be wandered.\n\n\
\x1b[90m  OFFLINE DEMO  ·  Five rooms to explore\n  Try look, north, east, up, who, or help.\x1b[0m\n\n";

const HELP: &str = "DEMO COMMANDS\n  look           Read the room again\n  n / s / e / w  Follow an exit\n  \
up / down      Take the tower stairs\n  who            See the demo cast\n  say <message>  Speak into the world\n  \
inventory      Check your pockets\n\nThis is a local, scripted sample. Connect to your own MUD\n\
from the world library for a real adventure.\n";

const WHO: &str = "OFFLINE DEMO CAST\n  You          A returning wanderer\n  Innkeeper    A scripted character\n\n\
No other players are connected.\n";

/// The five-room world and where the player stands.
#[derive(Clone, Debug, Default)]
pub struct DemoWorld {
    room: usize,
}

impl DemoWorld {
    /// How many rooms there are.
    pub const ROOMS: usize = ROOMS.len();

    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or restart) in the inn: the banner and the first room.
    pub fn start(&mut self) -> String {
        self.room = 0;
        format!("{BANNER}{}", self.describe())
    }

    /// The room the player is in (0 is the inn).
    pub fn room(&self) -> usize {
        self.room
    }

    /// The current room's name.
    pub fn room_name(&self) -> &'static str {
        ROOMS[self.room].name
    }

    /// The reply to one command, starting on a new line.
    pub fn command(&mut self, command: &str) -> String {
        let trimmed = command.trim();
        let lower = trimmed.to_lowercase();
        let value = match lower.as_str() {
            "n" => "north",
            "s" => "south",
            "e" => "east",
            "w" => "west",
            "u" => "up",
            "d" => "down",
            "l" => "look",
            other => other,
        };
        let reply = if let Some(&(_, next)) = ROOMS[self.room].exits.iter().find(|(d, _)| *d == value) {
            self.room = next;
            return format!("\n{}", self.describe());
        } else if value == "look" || value.is_empty() {
            return format!("\n{}", self.describe());
        } else if matches!(value, "north" | "south" | "east" | "west" | "up" | "down") {
            "You can't go that way.\n".to_string()
        } else if value == "help" {
            HELP.to_string()
        } else if value == "who" {
            WHO.to_string()
        } else if value == "inventory" || value == "i" {
            "You carry a brass compass, a folded map, and a little curiosity.\n".to_string()
        } else if let Some(said) = lower.strip_prefix("say ").map(|_| &trimmed[4..]) {
            let answer = if self.room == 0 {
                "The innkeeper smiles. \"Good to have a voice in this old place.\"\n"
            } else {
                "Your words mingle with the sound of the rain.\n"
            };
            format!("You say, \"{said}\"\n{answer}")
        } else {
            "That command isn't part of the demo. Try help.\n".to_string()
        };
        format!("\n{reply}\n> ")
    }

    fn describe(&self) -> String {
        let room = &ROOMS[self.room];
        let exits: Vec<&str> = room.exits.iter().map(|(d, _)| *d).collect();
        format!(
            "\x1b[1;38;2;229;192;123m{}\x1b[0m\n\x1b[90m{}\x1b[0m\n{}\n\n\x1b[36mExits: {}\x1b[0m\n\n> ",
            room.name,
            "─".repeat(42),
            room.description,
            exits.join("  ·  ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text without ANSI escapes, for comparing what a person reads.
    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn the_demo_opens_in_the_inn_with_the_banner() {
        let mut demo = DemoWorld::new();
        let text = plain(&demo.start());
        assert!(text.starts_with("  W A N D U R\n  A little world, waiting to be wandered."));
        assert!(text.contains("OFFLINE DEMO  ·  Five rooms to explore"));
        assert!(text.contains("The Lantern & the Rain\n"));
        assert!(text.contains("The old market is north of here, if you've a mind to wander.\""));
        assert!(text.ends_with("Exits: north  ·  east\n\n> "));
        assert_eq!(demo.room_name(), "The Lantern & the Rain");
    }

    /// Walk to every room and back, reading each description.
    #[test]
    fn a_walk_visits_every_room() {
        let mut demo = DemoWorld::new();
        demo.start();
        let mut seen = vec![demo.room()];
        for (command, room, words) in [
            ("north", 1, "a jar of white asters beneath the clock tower"),
            ("up", 3, "A swallow has made a home inside the silent bell."),
            ("d", 1, "The Old Market"),
            ("s", 0, "Somewhere, a kettle sings."),
            ("EAST", 2, "the forest breathes mist into the valley"),
            ("e", 4, "\"Every world begins with a first step.\""),
            ("w", 2, "A Bridge of Moss & Stone"),
            ("west", 0, "The Lantern & the Rain"),
        ] {
            let text = plain(&demo.command(command));
            assert_eq!(demo.room(), room, "{command}");
            assert!(text.contains(words), "{command}: {text}");
            assert!(text.starts_with('\n') && text.ends_with("\n\n> "));
            seen.push(room);
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), DemoWorld::ROOMS);
    }

    #[test]
    fn other_commands_answer_as_in_the_csharp_demo() {
        let mut demo = DemoWorld::new();
        demo.start();
        assert_eq!(plain(&demo.command("up")), "\nYou can't go that way.\n\n> ");
        assert!(plain(&demo.command("help")).contains("say <message>  Speak into the world"));
        assert!(plain(&demo.command("who")).contains("No other players are connected."));
        assert!(plain(&demo.command("i")).contains("a brass compass"));
        let said = plain(&demo.command("say Hello There"));
        assert!(
            said.contains("You say, \"Hello There\"\nThe innkeeper smiles."),
            "{said}"
        );
        demo.command("north");
        assert!(plain(&demo.command("say hi")).contains("Your words mingle with the sound of the rain."));
        assert!(plain(&demo.command("dance")).contains("That command isn't part of the demo. Try help."));
        assert!(plain(&demo.command("")).contains("The Old Market"));
        assert!(plain(&demo.command("l")).contains("The Old Market"));
    }
}
