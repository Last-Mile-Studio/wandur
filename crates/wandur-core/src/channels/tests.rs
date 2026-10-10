//! The C# `ChannelClassifierTests`, ported, and the session cases (GMCP beside patterns,
//! MSSP codebase, rules changed mid stream).

use std::time::{Duration, SystemTime};

use super::families::{family, for_world, match_codebase};
use super::rules::{ChannelRule, Matched, RuleSet, clean_speaker, validate};
use super::*;
use crate::protocol::parse_gmcp;

fn classify(rules: &RuleSet, line: &str) -> Option<Matched> {
    rules.match_line(line)
}

#[track_caller]
fn expect(rules: &RuleSet, line: &str, channel: &str, speaker: &str, text: &str) {
    let m = classify(rules, line).unwrap_or_else(|| panic!("no match for {line}"));
    assert_eq!(
        (m.channel.as_str(), m.speaker.as_str(), m.text.as_str()),
        (channel, speaker, text),
        "{line}"
    );
}

fn epoch() -> SystemTime {
    SystemTime::UNIX_EPOCH
}

fn feed(c: &mut Classifier, text: &str, at: SystemTime) -> Vec<ChannelLine> {
    let mut out = Vec::new();
    c.feed(text, at, &mut out);
    out
}

#[test]
fn smaug_recognizes_its_ooc_chat_newbie_and_tell_shapes() {
    let rules = family(Some("smaug"));
    expect(
        &rules,
        "[OOC] Aldric: anyone selling a lantern?",
        "ooc",
        "Aldric",
        "anyone selling a lantern?",
    );
    expect(
        &rules,
        "(OOC) Brenna: welcome back to the realms",
        "ooc",
        "Brenna",
        "welcome back to the realms",
    );
    expect(
        &rules,
        "Cadoc OOC: 'the east gate is closed again'",
        "ooc",
        "Cadoc",
        "the east gate is closed again",
    );
    expect(
        &rules,
        "Dwyn OOC: heading out for the night",
        "ooc",
        "Dwyn",
        "heading out for the night",
    );
    expect(
        &rules,
        "[CHAT] Eowyn: who wants to group up?",
        "chat",
        "Eowyn",
        "who wants to group up?",
    );
    expect(&rules, "(CHAT) Faelan: I am in", "chat", "Faelan", "I am in");
    expect(
        &rules,
        "Gorm CHAT: 'meet me at the fountain'",
        "chat",
        "Gorm",
        "meet me at the fountain",
    );
    expect(
        &rules,
        "[NEWBIE] Hilda: how do I wield a sword?",
        "newbie",
        "Hilda",
        "how do I wield a sword?",
    );
    expect(
        &rules,
        "(NEWBIE) Ivo: type wield sword",
        "newbie",
        "Ivo",
        "type wield sword",
    );
    expect(
        &rules,
        "Jorunn tells you 'bring the brass key'",
        "tell",
        "Jorunn",
        "bring the brass key",
    );
    expect(
        &rules,
        "Kel tells you: the north door is unlocked",
        "tell",
        "Kel",
        "the north door is unlocked",
    );
    expect(
        &rules,
        "Lira auctions 'a dented bronze helm'",
        "auction",
        "Lira",
        "a dented bronze helm",
    );
    expect(
        &rules,
        "Mabon shouts 'the river bridge is out'",
        "shout",
        "Mabon",
        "the river bridge is out",
    );
    assert!(classify(&rules, "Nessa says, 'good morning'").is_none());
    assert!(classify(&rules, "You are standing in a wide green field.").is_none());
}

#[test]
fn star_wars_reality_adds_its_own_shapes_on_top_of_smaug() {
    let rules = family(Some("swr"));
    expect(
        &rules,
        "[OOC] Aldric: anyone selling a lantern?",
        "ooc",
        "Aldric",
        "anyone selling a lantern?",
    );
    expect(
        &rules,
        "[OOC] (Xanto) may the force be with you",
        "ooc",
        "Xanto",
        "may the force be with you",
    );
    expect(
        &rules,
        "[SHIP] Talon: docking at bay three",
        "shipchannel",
        "Talon",
        "docking at bay three",
    );
    expect(
        &rules,
        "[CLAN] Vex: meeting at dawn",
        "clantalk",
        "Vex",
        "meeting at dawn",
    );
    expect(
        &rules,
        "Yara tells you 'the hangar is clear'",
        "tell",
        "Yara",
        "the hangar is clear",
    );
    expect(
        &rules,
        "Zev OOC: 'back in ten minutes'",
        "ooc",
        "Zev",
        "back in ten minutes",
    );
    expect(
        &rules,
        "[CHAT] Anka: who is flying tonight?",
        "chat",
        "Anka",
        "who is flying tonight?",
    );
    expect(
        &rules,
        "[NEWBIE] Bix: how do I board a ship?",
        "newbie",
        "Bix",
        "how do I board a ship?",
    );
    expect(
        &rules,
        "Cade tells you: land on pad two",
        "tell",
        "Cade",
        "land on pad two",
    );
    expect(&rules, "(OOC) Dax: good hunting", "ooc", "Dax", "good hunting");
    expect(&rules, "Enno shouts 'raid incoming'", "shout", "Enno", "raid incoming");
    expect(
        &rules,
        "(OOC) @Ryken: i had hibachi last week, it was excellent",
        "ooc",
        "Ryken",
        "i had hibachi last week, it was excellent",
    );
    expect(
        &rules,
        "(OOC) @Nield [IMM]: Now I'm hungry.",
        "ooc",
        "Nield",
        "Now I'm hungry.",
    );
    expect(
        &rules,
        "(OOC) @Fishy [NEW]: the natroll is waiting for me next tl",
        "ooc",
        "Fishy",
        "the natroll is waiting for me next tl",
    );
    expect(
        &rules,
        "(OOC) @Faern [RPC]: While I expect the pats to be better than the Jets most years",
        "ooc",
        "Faern",
        "While I expect the pats to be better than the Jets most years",
    );
    expect(
        &rules,
        "CommNet 0 [A Human male]( warmly ): Well played everyone!",
        "commnet",
        "A Human male",
        "Well played everyone!",
    );
    expect(
        &rules,
        "CommNet 0 [A Human female]: Yuriko? Are you on Ryloth and not busy?",
        "commnet",
        "A Human female",
        "Yuriko? Are you on Ryloth and not busy?",
    );
    assert!(classify(&rules, "Fenn says, 'nice ship'").is_none());
    assert!(
        classify(
            &rules,
            "If you have any questions, you can ask on the RPC or OOC channels."
        )
        .is_none()
    );
}

#[test]
fn diku_recognizes_gossip_group_and_tell_and_leaves_say_alone() {
    let rules = family(Some("diku"));
    expect(
        &rules,
        "Aric gossips, 'anyone need a cleric?'",
        "gossip",
        "Aric",
        "anyone need a cleric?",
    );
    expect(
        &rules,
        "Brann gossips 'I could use one too'",
        "gossip",
        "Brann",
        "I could use one too",
    );
    expect(
        &rules,
        "Cora tells you, 'follow me north'",
        "tell",
        "Cora",
        "follow me north",
    );
    expect(
        &rules,
        "Doran tells you 'wait by the gate'",
        "tell",
        "Doran",
        "wait by the gate",
    );
    expect(
        &rules,
        "Eda tells the group 'pulling now'",
        "group",
        "Eda",
        "pulling now",
    );
    expect(
        &rules,
        "Finn tells the group, 'ready when you are'",
        "group",
        "Finn",
        "ready when you are",
    );
    expect(
        &rules,
        "[Newbie] Gwen: where is the bank?",
        "newbie",
        "Gwen",
        "where is the bank?",
    );
    expect(
        &rules,
        "Hal auctions, 'a steel shield'",
        "auction",
        "Hal",
        "a steel shield",
    );
    expect(
        &rules,
        "Ivo auctions 'a rusty dagger'",
        "auction",
        "Ivo",
        "a rusty dagger",
    );
    expect(
        &rules,
        "Jory shouts, 'help in the crypt'",
        "shout",
        "Jory",
        "help in the crypt",
    );
    expect(&rules, "Kara shouts 'on my way'", "shout", "Kara", "on my way");
    assert!(classify(&rules, "Lem says, 'hello there'").is_none());
    assert!(classify(&rules, "Mira says 'watch your step'").is_none());
}

#[test]
fn rom_adds_question_and_answer_to_the_diku_shapes() {
    let rules = family(Some("rom"));
    expect(
        &rules,
        "Mira questions 'where is the smith?'",
        "question",
        "Mira",
        "where is the smith?",
    );
    expect(
        &rules,
        "Nolan answers 'just south of the square'",
        "answer",
        "Nolan",
        "just south of the square",
    );
    expect(
        &rules,
        "Orin MUSIC: 'a tune from the north'",
        "music",
        "Orin",
        "a tune from the north",
    );
    expect(&rules, "Pell gossips 'good evening'", "gossip", "Pell", "good evening");
    expect(&rules, "Quill gossips, 'and to you'", "gossip", "Quill", "and to you");
    expect(
        &rules,
        "Rhys tells you 'meet me at the inn'",
        "tell",
        "Rhys",
        "meet me at the inn",
    );
    expect(
        &rules,
        "Sena tells you, 'the guard is watching'",
        "tell",
        "Sena",
        "the guard is watching",
    );
    expect(
        &rules,
        "Toran tells the group 'healing up'",
        "group",
        "Toran",
        "healing up",
    );
    expect(
        &rules,
        "[Newbie] Ura: how do I recall?",
        "newbie",
        "Ura",
        "how do I recall?",
    );
    expect(
        &rules,
        "Vann shouts 'the gates are open'",
        "shout",
        "Vann",
        "the gates are open",
    );
    expect(
        &rules,
        "Wend auctions 'a plain iron ring'",
        "auction",
        "Wend",
        "a plain iron ring",
    );
    assert!(classify(&rules, "Yorick says, 'welcome traveller'").is_none());
}

#[test]
fn circle_adds_holler_and_congrat_to_the_diku_shapes() {
    let rules = family(Some("circle"));
    expect(
        &rules,
        "Orin hollers, 'everyone out of the tower'",
        "holler",
        "Orin",
        "everyone out of the tower",
    );
    expect(
        &rules,
        "Pia congrats, 'well done on thirty'",
        "congrat",
        "Pia",
        "well done on thirty",
    );
    expect(&rules, "Quen gossips, 'morning all'", "gossip", "Quen", "morning all");
    expect(&rules, "Rolf gossips 'morning'", "gossip", "Rolf", "morning");
    expect(
        &rules,
        "Sig tells you, 'the shop is closed'",
        "tell",
        "Sig",
        "the shop is closed",
    );
    expect(
        &rules,
        "Tam tells you 'try again tomorrow'",
        "tell",
        "Tam",
        "try again tomorrow",
    );
    expect(
        &rules,
        "Ulla tells the group 'buffing now'",
        "group",
        "Ulla",
        "buffing now",
    );
    expect(
        &rules,
        "[Newbie] Vala: what does score show?",
        "newbie",
        "Vala",
        "what does score show?",
    );
    expect(
        &rules,
        "Wim auctions, 'a soft leather cap'",
        "auction",
        "Wim",
        "a soft leather cap",
    );
    expect(
        &rules,
        "Xan shouts, 'guards to the east gate'",
        "shout",
        "Xan",
        "guards to the east gate",
    );
    expect(&rules, "Yara shouts 'coming'", "shout", "Yara", "coming");
    assert!(classify(&rules, "Zeno says, 'mind the step'").is_none());
}

#[test]
fn lp_recognizes_bracketed_channels_on_either_side_of_the_name() {
    let rules = family(Some("lp"));
    expect(&rules, "[chat] Perrin: evening all", "chat", "Perrin", "evening all");
    expect(&rules, "Quinn [chat]: evening", "chat", "Quinn", "evening");
    expect(
        &rules,
        "[newbie] Rhea: how do I look at things?",
        "newbie",
        "Rhea",
        "how do I look at things?",
    );
    expect(&rules, "Sten [newbie]: type look", "newbie", "Sten", "type look");
    expect(
        &rules,
        "[ooc] Tova: back in a moment",
        "ooc",
        "Tova",
        "back in a moment",
    );
    expect(&rules, "Ulf [ooc]: no rush", "ooc", "Ulf", "no rush");
    expect(
        &rules,
        "Vidar tells you: meet me at the well",
        "tell",
        "Vidar",
        "meet me at the well",
    );
    expect(&rules, "Wren tells you 'on my way'", "tell", "Wren", "on my way");
    expect(
        &rules,
        "[Chat] Yvo: the guild hall moved",
        "chat",
        "Yvo",
        "the guild hall moved",
    );
    expect(&rules, "[NEWBIE] Zara: thank you", "newbie", "Zara", "thank you");
    assert!(classify(&rules, "Alva says: welcome").is_none());
    assert!(classify(&rules, "The wizard smiles at you.").is_none());
}

#[test]
fn generic_keeps_only_the_shapes_that_are_safe_across_codebases() {
    let rules = family(None);
    expect(&rules, "[OOC] Aldric: good evening", "ooc", "Aldric", "good evening");
    expect(&rules, "(OOC) Brenna: and to you", "ooc", "Brenna", "and to you");
    expect(&rules, "[CHAT] Cass: anyone around?", "chat", "Cass", "anyone around?");
    expect(&rules, "(CHAT) Dara: over here", "chat", "Dara", "over here");
    expect(
        &rules,
        "[NEWBIE] Edda: how do I get help?",
        "newbie",
        "Edda",
        "how do I get help?",
    );
    expect(&rules, "(NEWBIE) Fell: type help", "newbie", "Fell", "type help");
    expect(
        &rules,
        "Gale gossips, 'the market is open'",
        "gossip",
        "Gale",
        "the market is open",
    );
    expect(&rules, "Hune gossips 'thanks'", "gossip", "Hune", "thanks");
    expect(
        &rules,
        "Idris tells you 'meet me inside'",
        "tell",
        "Idris",
        "meet me inside",
    );
    expect(
        &rules,
        "Jek tells you, 'the door sticks'",
        "tell",
        "Jek",
        "the door sticks",
    );
    expect(&rules, "Kile tells you: on my way", "tell", "Kile", "on my way");
    assert!(classify(&rules, "Lorn says, 'good hunting'").is_none());
    // A shape only some codebases use stays out of the generic set rather than guessing.
    assert!(classify(&rules, "Mave hollers, 'fire!'").is_none());
}

#[test]
fn tells_and_pages_are_marked_private_and_open_channels_are_not() {
    let rules = family(Some("smaug"));
    assert!(
        classify(&rules, "Jorunn tells you 'this is between us'")
            .unwrap()
            .private
    );
    assert_eq!(
        classify(&rules, "Jorunn tells you 'x'")
            .unwrap()
            .reply_command
            .as_deref(),
        Some("tell {speaker}")
    );
    assert!(!classify(&rules, "[OOC] Aldric: hello").unwrap().private);
    assert_eq!(
        classify(&rules, "[OOC] Aldric: hello")
            .unwrap()
            .reply_command
            .as_deref(),
        Some("ooc")
    );
}

#[test]
fn the_first_rule_that_matches_wins_and_a_line_is_never_two_messages() {
    let rules = RuleSet::new([
        ChannelRule::new("first", r"\[OOC\] (?<speaker>[A-Za-z]+): (?<text>.*)$", Some("one")),
        ChannelRule::new("second", r"\[OOC\] (?<speaker>[A-Za-z]+): (?<text>.*)$", Some("two")),
    ]);
    let m = classify(&rules, "[OOC] Aldric: hello").unwrap();
    assert_eq!(m.channel, "first");
    assert_eq!(m.reply_command.as_deref(), Some("one"));
}

#[test]
fn a_multi_word_speaker_is_prose_rather_than_a_channel() {
    let rules = family(Some("diku"));
    assert!(classify(&rules, "The town crier gossips, 'hear ye'").is_none());
    assert!(classify(&rules, "Crier gossips, 'hear ye'").is_some());
}

#[test]
fn color_codes_are_stripped_before_matching_and_kept_for_the_body() {
    let mut c = Classifier::new(family(Some("smaug")));
    let lines = feed(
        &mut c,
        "\u{1b}[1;33m[OOC] Bora:\u{1b}[0;32m hello there\u{1b}[0m\r\n",
        epoch(),
    );
    assert_eq!(lines.len(), 1);
    let m = &lines[0].message;
    assert_eq!(
        (m.channel.as_str(), m.speaker.as_str(), m.text.as_str()),
        ("ooc", "Bora", "hello there")
    );
    assert!(!m.raw_line.contains('\u{1b}'));
    assert_eq!(strip_ansi(&m.body).trim(), "hello there");
    assert!(m.body.contains("\u{1b}[0;32m"), "the colours stay: {:?}", m.body);
}

#[test]
fn an_indented_following_line_extends_the_message_above_it() {
    let mut c = Classifier::new(family(Some("smaug")));
    assert_eq!(
        feed(&mut c, "[OOC] Aldric: I had a longer thought about\r\n", epoch()).len(),
        1
    );
    let continued = feed(&mut c, "     the price of lanterns lately\r\n", epoch());
    assert_eq!(continued.len(), 1);
    assert!(continued[0].continuation);
    assert_eq!(
        continued[0].message.text,
        "I had a longer thought about the price of lanterns lately"
    );
    // A line back at the margin is ordinary output again, not more of the conversation.
    assert!(feed(&mut c, "A cold wind blows.\r\n", epoch()).is_empty());
    assert!(feed(&mut c, "     and this is no longer a channel line\r\n", epoch()).is_empty());
}

#[test]
fn only_complete_lines_are_classified() {
    let mut c = Classifier::new(family(Some("smaug")));
    assert!(feed(&mut c, "[OOC] Aldric: half a ", epoch()).is_empty());
    let lines = feed(&mut c, "line arrives later\r\n", epoch());
    assert_eq!(lines[0].message.text, "half a line arrives later");
}

#[test]
fn a_structured_message_suppresses_the_printed_copy_of_the_same_line() {
    let mut c = Classifier::new(family(Some("smaug")));
    let now = epoch();
    c.expect_printed_copy("[OOC] Aldric: hello", now);
    assert!(feed(&mut c, "[OOC] Aldric: hello\r\n", now).is_empty());
    // A different line, and the same line a minute later, are both real output.
    c.expect_printed_copy("[OOC] Aldric: hello", now);
    assert_eq!(feed(&mut c, "[OOC] Brenna: hello\r\n", now).len(), 1);
    c.expect_printed_copy("[OOC] Aldric: hello", now);
    assert_eq!(
        feed(&mut c, "[OOC] Aldric: hello\r\n", now + Duration::from_secs(60)).len(),
        1
    );
}

#[test]
fn gmcp_channel_messages_decode_to_a_channel_speaker_and_line() {
    let m =
        parse_gmcp(br#"Comm.Channel.Text {"channel":"ooc","talker":"Aldric","text":"[OOC] Aldric: hello"}"#).unwrap();
    assert_eq!(
        decode(&m),
        Some(("ooc".into(), "Aldric".into(), "[OOC] Aldric: hello".into()))
    );
    assert!(decode(&parse_gmcp(br#"Room.Info {"name":"A field"}"#).unwrap()).is_none());
    assert!(decode(&parse_gmcp(b"Comm.Channel.Text not json").unwrap()).is_none());
    assert!(decode(&parse_gmcp(br#"Comm.Channel.Text {"channel":"ooc"}"#).unwrap()).is_none());
}

#[test]
fn the_directorys_codebase_chooses_the_family() {
    for (codebase, family) in [
        ("SMAUG 1.4a", "smaug"),
        ("SWR 1.0 (SMAUG derivative)", "swr"),
        ("ROM 2.4b6", "rom"),
        ("CircleMUD 3.1", "circle"),
        ("DikuMUD", "diku"),
        ("LPMud", "lp"),
        ("custom in house engine", "generic"),
        ("", "generic"),
    ] {
        assert_eq!(match_codebase(Some(codebase)), family, "{codebase}");
    }
    assert_eq!(match_codebase(None), "generic");
    assert_eq!(
        families::names(),
        ["smaug", "swr", "diku", "rom", "circle", "lp", "generic"]
    );
}

#[test]
fn a_worlds_own_rules_come_before_the_family_defaults() {
    let own = [ChannelRule::new("house", "House: (?<text>.*)$", Some("house"))];
    let rules = for_world(&own, Some("SMAUG 1.4a"));
    assert_eq!(rules.len(), family(Some("smaug")).len() + 1);
    assert_eq!(rules.rules()[0].channel, "house");
    assert_eq!(classify(&rules, "House: the door is open").unwrap().channel, "house");
    // The family still recognises what the world did not teach.
    assert_eq!(classify(&rules, "[OOC] Aldric: hello").unwrap().channel, "ooc");
    assert!(for_world(&[], Some("SMAUG 1.4a")).len() > 1);
}

#[test]
fn a_world_rule_beats_the_family_rule_for_the_same_line_and_an_exclusion_beats_both() {
    let taught = ChannelRule::new("staff", r"\[OOC\] (?<speaker>[A-Za-z]+): (?<text>.*)$", Some("ooc"));
    let rules = for_world(std::slice::from_ref(&taught), Some("SMAUG 1.4a"));
    expect(&rules, "[OOC] Aldric: hello", "staff", "Aldric", "hello");

    // An exclusion anywhere in the set wins, whether the line was claimed by the world or the
    // family.
    let excluded = for_world(
        &[
            taught.clone(),
            ChannelRule::exclusion("ooc", r"\[OOC\] Aldric: "),
            ChannelRule::exclusion("tell", "[A-Za-z]+ tells you 'the weather"),
        ],
        Some("SMAUG 1.4a"),
    );
    assert!(classify(&excluded, "[OOC] Aldric: hello").is_none());
    assert!(classify(&excluded, "Jorunn tells you 'the weather is fine'").is_none());
    expect(&excluded, "[OOC] Brenna: hello", "staff", "Brenna", "hello");
    expect(
        &excluded,
        "Jorunn tells you 'bring the brass key'",
        "tell",
        "Jorunn",
        "bring the brass key",
    );
    let first: Vec<&str> = excluded.rules().iter().take(3).map(|r| r.channel.as_str()).collect();
    assert_eq!(first, ["staff", "ooc", "tell"]);
    assert!(!family(Some("smaug")).channels().contains(&"staff".to_string()));
    assert!(excluded.channels().contains(&"staff".to_string()));

    // A disabled rule stays on the list and does nothing.
    let disabled = for_world(
        &[ChannelRule {
            disabled: true,
            ..taught
        }],
        Some("SMAUG 1.4a"),
    );
    expect(&disabled, "[OOC] Aldric: hello", "ooc", "Aldric", "hello");
    assert!(disabled.rules().iter().any(|r| r.channel == "staff" && r.disabled));
    assert!(!disabled.channels().contains(&"staff".to_string()));
}

#[test]
fn a_taught_speaker_loses_its_account_mark_role_tag_and_brackets() {
    let rules = RuleSet::new([
        ChannelRule::new(
            "ooc",
            r"\(OOC\) (?<speaker>@?[A-Za-z]+(?: \[[A-Za-z]+\])?): (?<text>.*)$",
            Some("ooc"),
        ),
        ChannelRule::new(
            "commnet",
            r"CommNet [0-9]+ (?<speaker>\[[^\]]+\])(?:\( ?[^)]*? ?\))?: (?<text>.*)$",
            Some("commnet"),
        ),
    ]);
    expect(
        &rules,
        "(OOC) @Nield [IMM]: Now I'm hungry.",
        "ooc",
        "Nield",
        "Now I'm hungry.",
    );
    expect(&rules, "(OOC) Aldric: hello", "ooc", "Aldric", "hello");
    expect(
        &rules,
        "CommNet 0 [A Human male]( warmly ): Well played everyone!",
        "commnet",
        "A Human male",
        "Well played everyone!",
    );
    assert_eq!(clean_speaker("@Nield [IMM]"), "Nield");
    assert_eq!(clean_speaker("[A Human male]"), "A Human male");
    assert_eq!(clean_speaker("Aldric"), "Aldric");
}

#[test]
fn a_rule_serializes_in_the_shape_a_channel_pack_carries() {
    assert_eq!(
        serde_json::to_string(&ChannelRule::new("ooc", r"\(OOC\) .*$", Some("ooc"))).unwrap(),
        r#"{"channel":"ooc","pattern":"\\(OOC\\) .*$","reply_command":"ooc"}"#
    );
    let all = ChannelRule {
        channel: "tell".into(),
        pattern: "x".into(),
        reply_command: Some("tell {speaker}".into()),
        private: true,
        exclude: true,
        disabled: true,
    };
    assert_eq!(
        serde_json::to_string(&all).unwrap(),
        r#"{"channel":"tell","pattern":"x","reply_command":"tell {speaker}","private":true,"exclude":true,"disabled":true}"#
    );
    let parsed: ChannelRule = serde_json::from_str(
        r#"{"channel":"chat","pattern":"y","reply_command":"chat","private":false,"exclude":true}"#,
    )
    .unwrap();
    assert_eq!(
        parsed,
        ChannelRule {
            exclude: true,
            ..ChannelRule::new("chat", "y", Some("chat"))
        }
    );
    // The saved world carries the same shape, so a pack's list drops straight onto a world.
    let world = crate::settings::SavedWorld {
        name: "Test".into(),
        host: "example.org".into(),
        channel_rules: vec![parsed.clone()],
        ..Default::default()
    };
    let json = serde_json::to_string(&world).unwrap();
    assert!(
        json.contains(r#""channel_rules":[{"channel":"chat","pattern":"y","reply_command":"chat","exclude":true}]"#),
        "{json}"
    );
    let back: crate::settings::SavedWorld = serde_json::from_str(&json).unwrap();
    assert_eq!(back.channel_rules, [parsed]);
}

#[test]
fn an_unreadable_pattern_is_ignored_rather_than_fatal() {
    let rules = RuleSet::new([
        ChannelRule::new("broken", "(?<speaker>[A-Za-z", None),
        ChannelRule::new("ooc", r"\[OOC\] (?<speaker>[A-Za-z]+): (?<text>.*)$", Some("ooc")),
    ]);
    assert_eq!(rules.len(), 1);
    assert_eq!(classify(&rules, "[OOC] Aldric: hello").unwrap().channel, "ooc");
}

#[test]
fn a_world_rejects_channel_rules_it_could_not_use() {
    let mut world = crate::settings::SavedWorld {
        name: "Test".into(),
        host: "example.org".into(),
        channel_rules: vec![ChannelRule::new("ooc", "(?<speaker>[A-Za-z", None)],
        ..Default::default()
    };
    assert!(world.validate().is_err());
    assert!(validate(&world.channel_rules).is_err());
    world.codebase = "SMAUG 1.4a".into();
    world.channel_rules = vec![ChannelRule::new("ooc", r"^\[OOC\] ", Some("ooc"))];
    world.validate().unwrap();
}

fn gmcp(text: &str) -> crate::protocol::GmcpMessage {
    parse_gmcp(text.as_bytes()).unwrap()
}

/// A world that names its channels over GMCP and prints them too: each message lands once,
/// on the channel the package names, and the pattern that also fits the printed line neither
/// duplicates it nor moves it.
#[test]
fn gmcp_channels_keep_working_and_are_not_duplicated_by_a_pattern_match() {
    let mut session = SessionChannels::new(Vec::new(), "SMAUG 1.4a");
    let now = epoch() + Duration::from_secs(1_000);
    assert!(session.receive_gmcp(
        &gmcp(
            r#"Comm.Channel.Text {"channel":"chat","talker":"Brenna","text":"[CHAT] Brenna: who wants to group up?"}"#
        ),
        now,
    ));
    session.receive_text("[CHAT] Brenna: who wants to group up?\r\n", now);
    // A GMCP channel the family has no rule for, printed in a shape the family does know.
    assert!(session.receive_gmcp(
        &gmcp(r#"Comm.Channel.Text {"channel":"guild","talker":"Wren","text":"[OOC] Wren: guild talk"}"#),
        now,
    ));
    session.receive_text("[OOC] Wren: guild talk\r\n", now);
    let tabs: Vec<(&str, usize)> = session
        .log
        .tabs()
        .iter()
        .map(|t| (t.title(), t.messages.len()))
        .collect();
    assert_eq!(tabs[1..], [("chat", 1), ("guild", 1)]);
    assert_eq!(session.log.tabs()[0].messages.len(), 2);
    let chat = &session.log.tabs()[1].messages[0];
    assert_eq!(
        (chat.speaker.as_str(), chat.text.as_str()),
        ("Brenna", "who wants to group up?")
    );
    assert_eq!(strip_ansi(&chat.body), "who wants to group up?");
    assert_eq!(
        chat.reply_command.as_deref(),
        Some("chat"),
        "from the family's chat rule"
    );
    // The same line printed later is a new message (another person said it again).
    session.receive_text(
        "[CHAT] Brenna: who wants to group up?\r\n",
        now + Duration::from_secs(5),
    );
    assert_eq!(session.log.tabs()[1].messages.len(), 2);
}

#[test]
fn the_server_codebase_picks_the_family_only_while_the_world_names_none() {
    let mut session = SessionChannels::new(Vec::new(), "");
    session.receive_text("Aric gossips, 'anyone need a cleric?'\r\n", epoch());
    session.receive_text("Mira questions 'where is the smith?'\r\n", epoch());
    assert_eq!(session.log.tabs().len(), 2, "generic knows gossip, not question");
    session.use_server_codebase("ROM 2.4b6");
    assert_eq!(session.codebase(), Some("ROM 2.4b6"));
    session.receive_text("Mira questions 'where is the smith?'\r\n", epoch());
    assert_eq!(session.log.tabs().len(), 3);

    let mut named = SessionChannels::new(Vec::new(), "LPMud");
    named.use_server_codebase("ROM 2.4b6");
    assert_eq!(named.codebase(), Some("LPMud"), "the world's own codebase wins");
    named.receive_text("Mira questions 'where is the smith?'\r\n", epoch());
    assert!(named.log.is_empty());
}

#[test]
fn rules_changed_mid_line_apply_to_that_line_and_privacy_drops_it() {
    let mut session = SessionChannels::new(Vec::new(), "SMAUG 1.4a");
    session.receive_text("[CLAN] Vex: meeting ", epoch());
    let clan = ChannelRule::new("clan", r"^\[CLAN\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$", Some("clan"));
    session.configure(std::slice::from_ref(&clan), "SMAUG 1.4a");
    session.receive_text("at dawn\r\n", epoch());
    assert_eq!(session.log.tabs()[1].channel, "clan");
    assert_eq!(session.log.tabs()[1].messages[0].text, "meeting at dawn");
    // A private stretch ends the line in flight.
    session.receive_text("[CLAN] Vex: my password is ", epoch());
    session.reset();
    session.receive_text("hunter2\r\n", epoch());
    assert_eq!(session.log.tabs()[0].messages.len(), 1);
}

#[test]
fn a_continuation_replaces_the_last_message_and_is_not_unread_again() {
    let mut session = SessionChannels::new(Vec::new(), "SMAUG 1.4a");
    session.receive_text(
        "[OOC] Aldric: I had a longer thought about\r\n     lanterns\r\n",
        epoch(),
    );
    let all = &session.log.tabs()[0];
    assert_eq!(all.messages.len(), 1);
    assert_eq!(all.messages[0].text, "I had a longer thought about lanterns");
    assert_eq!(session.log.tabs()[1].unread, 1);
}
