// Builds a synthetic C# Wandur data directory with the client's own stores, so the schema and
// every JSON payload are exactly what the C# client writes. Nothing here is real data: the
// worlds, hosts, names and text are invented, and no password or API key is stored anywhere.
//
// Usage: dotnet run --project tools/FixtureGen -- <output directory>
using Microsoft.Data.Sqlite;
using Wandur.Core.Agents;
using Wandur.Core.Channels;
using Wandur.Core.History;
using Wandur.Core.Mapping;
using Wandur.Core.Scripting;
using Wandur.Core.Settings;
using Wandur.Core.Storage;

var output = Path.GetFullPath(args.Length > 0 ? args[0] : "csharp-data");
if (Directory.Exists(output)) Directory.Delete(output, recursive: true);
Directory.CreateDirectory(output);
var dbPath = Path.Combine(output, "wandur.db");
var database = new ClientDatabase(dbPath);
var clock = new FixedClock(new DateTimeOffset(2026, 9, 1, 18, 0, 0, TimeSpan.Zero));

// Saved worlds and settings ------------------------------------------------------------------
var lanternId = Guid.Parse("11111111-2222-4333-8444-555555555501");
var harborId = Guid.Parse("11111111-2222-4333-8444-555555555502");
var mossId = Guid.Parse("11111111-2222-4333-8444-555555555503");
var keepId = Guid.Parse("11111111-2222-4333-8444-555555555504");
var lanternPassword = Guid.Parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01");
var harborPassword = Guid.Parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee02");
var customTheme = new UserTheme
{
    Id = "custom-0123456789abcdef0123456789abcdef",
    Name = "Fixture Dusk",
    IsLight = false,
    Colors = UserTheme.ColorKeys.Select((key, i) => (key, value: $"#{(0x20 + i * 7):X2}{(0x30 + i * 5):X2}{(0x40 + i * 3):X2}"))
        .ToDictionary(p => p.key, p => p.value),
    AnsiColors = new() { [1] = "#CC5555", [4] = "#5577DD" },
};
var settingsStore = new SqliteSettingsStore(database, Path.Combine(output, "settings.json"));
var lanternOld = new ConnectionProfile
{
    Id = lanternId, Name = "Fixture Lantern Road", Host = "old-lantern.fixture.example", Port = 4000,
    Username = "wayfarer", PasswordId = lanternPassword, AutoLogin = true,
};
// First saved under its old address; editing the address below keeps the world and leaves the
// old endpoint as an alias.
settingsStore.Save(new ClientSettings { Profiles = [lanternOld] });
var lantern = lanternOld with
{
    Host = "lantern.fixture.example",
    Codebase = "SMAUG 1.4a",
    ChannelRules = new ChannelRuleList([
        new ChannelRule("gossip", @"^(?<speaker>\w+) gossips, '(?<text>.*)'$", "gossip"),
        new ChannelRule("tells", @"^(?<speaker>\w+) tells you, '(?<text>.*)'$", "tell {speaker}", IsPrivate: true),
        // .NET look-behind: the Rust client's regex syntax cannot run it.
        new ChannelRule("ooc", @"^(?<=\[)OOC\] (?<speaker>\w+): (?<text>.*)$", "ooc"),
        new ChannelRule("gossip", @"^Fixture bot gossips", Exclude: true),
    ]),
};
var harbor = new ConnectionProfile
{
    Id = harborId, Name = "Fixture Glass Harbor", Host = "harbor.fixture.example", Port = 6697, UseTls = true,
    Encoding = "latin1", Username = "Quill", PasswordId = harborPassword,
    Theme = System.Text.Json.JsonSerializer.Deserialize<Wandur.Core.Discovery.WorldTheme>("""
        {"version":1,"id":"fixture-harbor","name":"Fixture Harbor","variant":"dark","corner_radius":6,
         "colors":{"shell":"#101820","panel":"#18222C","terminal":"#0C1218","text":"#E0E8F0","muted":"#8090A0",
                   "accent":"#40C0E0","accent_secondary":"#E0C040","border":"#2A3A4A","terminal_text":"#D0E0F0"}}
        """),
};
var moss = new ConnectionProfile { Id = mossId, Name = "Fixture Moss Hollow", Host = "203.0.113.7", Port = 2323 };
var keep = new ConnectionProfile { Id = keepId, Name = "Fixture Ash Keep", Host = "2001:db8::5", Port = 4000, Encoding = "utf-8" };
settingsStore.Save(new ClientSettings
{
    Theme = customTheme.Id, Skin = "Armored", Language = "fr", FontSize = 17, Foreground = "#E0E0E0",
    LocalEcho = true, AllowBlinkingText = true, ScrollTailShare = 0.3, UseWorldThemes = false,
    ClassifyRoomsLocally = false, RoomClassificationThreshold = 0.85, MapAutoCenter = false, ShowChannelsPanel = false,
    ComposerSuggestions = false, HistoryEnabled = true, HideHistoryRecordingNotice = true, CheckForUpdates = false,
    InstallId = Guid.Parse("99999999-8888-4777-8666-555555555555"), SendInstallId = false,
    LastUpdateCheck = new Wandur.Core.Updates.UpdateCheckRecord { CheckedAt = clock.GetUtcNow(), Version = "0.1.6" },
    SkippedUpdateVersion = "0.1.5", HistoryRetentionDays = 90, CustomThemes = [customTheme],
    Profiles = [lantern, harbor, moss, keep],
});

// Usage: connections over a few weeks, and the character last played -------------------------
var usage = new SqliteWorldUsageStore(database, clock);
foreach (var day in new[] { 1, 3, 8, 20 })
{
    clock.Now = new DateTimeOffset(2026, 9, day, 19, 30, 0, TimeSpan.Zero);
    usage.RecordConnection(lantern.Host, lantern.Port);
}
usage.RecordCharacter(lantern.Host, lantern.Port, "Wayfarer");
clock.Now = new DateTimeOffset(2026, 9, 21, 8, 15, 0, TimeSpan.Zero);
usage.RecordConnection(harbor.Host, harbor.Port);
usage.RecordCharacter(keep.Host, keep.Port, "Ashen");

// Script libraries: scripts and macros ------------------------------------------------------
var scripts = new SqliteWorldScriptLibraryStore(database, Path.Combine(output, "scripts"));
var lanternKey = lantern.Host + ":" + lantern.Port;
scripts.Load(lanternKey); // gives the starter script, as the client does on first open
scripts.Upsert(lanternKey, new(Guid.Parse("cccccccc-0000-4000-8000-000000000001"), "Fixture greeter",
    "mud.on(Events.Line, line => { if (line.text.includes('fixture bell')) mud.send('wave'); });", Enabled: true));
foreach (var (n, name, macro) in new (int, string, MacroDefinition)[]
{
    (2, "Fixture trigger", new(MacroKind.Trigger, "You are hungry", "eat bread\ndrink water", MacroMatch.StartsWith, IgnoreCase: true)),
    (3, "Fixture alias", new(MacroKind.Alias, "gh", "go home")),
    (4, "Fixture timer", new(MacroKind.Timer, "", "save", IntervalSeconds: 300)),
    (5, "Fixture key", new(MacroKind.Shortcut, "F2", "look")),
})
    scripts.Upsert(lanternKey, new(Guid.Parse($"cccccccc-0000-4000-8000-00000000000{n}"), name, MacroCompiler.Compile(macro), Enabled: true, Macro: macro));
var doomed = Guid.Parse("cccccccc-0000-4000-8000-000000000009");
scripts.Upsert(lanternKey, new(doomed, "Fixture removed", "// gone", Enabled: false));
scripts.Delete(lanternKey, doomed);
// The TLS world's library is opened by its legacy key with the C# ":True" suffix.
var harborLegacyKey = harbor.Host + ":" + harbor.Port + ":True";
scripts.Load(harborLegacyKey);
var pack = new ScriptPackInfo("fixture-pack", ScriptPackInfo.Reviewed, 3, "Fixture pack from the directory");
scripts.Upsert(harborLegacyKey, new(ScriptPackInfo.IdFor(harbor.Host + ":" + harbor.Port, pack.PackId), "Fixture pack script",
    "mud.on(Events.Gmcp, msg => {});", Enabled: true) { Pack = pack, AllowSend = true });

// Agent profiles -----------------------------------------------------------------------------
var agents = new SqliteAgentProfileStore(database);
agents.Save(lanternKey, new AgentProfile
{
    Id = Guid.Parse("dddddddd-0000-4000-8000-000000000001"), Provider = "lmstudio-native", Endpoint = "http://127.0.0.1:1234",
    Model = "fixture-model", SystemPrompt = "Fixture prompt.",
    Goals = [new AgentGoal(Guid.Parse("dddddddd-0000-4000-8000-0000000000a1"), "Explore the fixture road.") { Name = "Explore", Rules = "Never attack." },
             new AgentGoal(Guid.Parse("dddddddd-0000-4000-8000-0000000000a2"), "Rest at the inn.", Enabled: false) { Name = "Rest" }],
    Commands = "look | look | Observe\nnorth | north | Move north",
    MaxDecisions = 12, JsonMode = true,
    // A reference to a key that would live in the vault; the fixture stores no key.
    CredentialId = Guid.Parse("dddddddd-0000-4000-8000-0000000000c1"),
});
agents.Load(harbor.Host + ":" + harbor.Port); // defaults, stored once

// Maps -----------------------------------------------------------------------------------------
var maps = new SqliteRoomMapStore(database, Path.Combine(output, "maps"));
MapRoom Room(string id, string name, string area, double x, double y, double z) =>
    new(id, name, $"The {name.ToLowerInvariant()} of the fixture.", area, x, y, z, false, id.StartsWith("gmcp:") ? id[5..] : null)
    { ObservedName = name, ObservedArea = area, KnownExits = ["north", "south"], Revision = 1 };
var rooms = new List<MapRoom>
{
    Room("gmcp:1001", "Fixture Square", "Fixture Town", 0, 0, 0),
    Room("gmcp:1002", "Fixture Gate", "Fixture Town", 0, -1, 0),
    Room("gmcp:1003", "Fixture Cellar", "Fixture Town", 0, 0, -1) with { Environment = "Underground", Color = "#665544", Symbol = "C" },
    Room("gmcp:2001", "Fixture Shore", "Fixture Coast", 5, 2, 0) with { InferredEnvironment = "beach", InferredConfidence = 0.91, InferredKey = "k1" },
    // Edited by hand and locked in place.
    Room("gmcp:2002", "Fixture Lighthouse", "Fixture Coast", 6, 2, 1) with { IsManuallyEdited = true, IsLocked = true, Notes = "Fixture note.", Weight = 3, Revision = 4 },
    // A section parked far away, as an unplaced teleport leaves it.
    Room("text:fixture-far", "Fixture Far Glade", "Fixture Coast", 1000, 1000, 0) with { Provisional = true },
};
var links = new List<MapLink>
{
    new("gmcp:1001", "gmcp:1002", "north", true),
    new("gmcp:1002", "gmcp:1001", "south", true) { DoorState = MapDoorState.Locked },
    new("gmcp:1001", "gmcp:1003", "down", true) { Command = "climb down", Weight = 2 },
    new("gmcp:1001", "gmcp:2001", "east", false) { LinePoints = [new(2, 0), new(4, 2)] },
    new("gmcp:2001", "gmcp:2002", "up", true) { IsManuallyEdited = true, IsLocked = true, Revision = 3 },
};
maps.Save(lantern.Host, lantern.Port, new MapSnapshot(rooms, links, [], null, MapTrackingState.Unknown, RoomDataSource.Gmcp, 42)
{
    AreaSettings = [new MapAreaSettings("Fixture Coast", GridMode: true) { Revision = 2 }],
    RoomAliases = [new MapRoomAlias("text:fixture-old-square", "gmcp:1001", 2)],
    DeletedRooms = [new MapRoomDeletion("gmcp:9999", 5)],
    DeletedLinks = [new MapLinkDeletion("gmcp:1003", "up", 6)],
});
maps.Save(harbor.Host, harbor.Port, new MapSnapshot(
    [Room("text:harbor-dock", "Fixture Dock", "Fixture Harbor", 0, 0, 0)], [], [], null, MapTrackingState.Unknown, RoomDataSource.Text, 1));

// Session history -----------------------------------------------------------------------------
var history = new SqliteHistoryStore(database);
var t0 = new DateTimeOffset(2026, 9, 20, 19, 30, 0, TimeSpan.Zero);
var s1 = new HistorySession("eeeeeeee000040008000000000000001", ClientDatabase.CanonicalEndpoint(lanternKey), lantern.Name, "Wayfarer", t0, t0.AddMinutes(40));
history.Append(s1, [
    new(0, t0, "received", "Welcome to the fixture lantern road."),
    new(1, t0.AddSeconds(5), "private", ""),
    new(2, t0.AddSeconds(9), "received", "The fixture bell rings softly in the square."),
    new(3, t0.AddSeconds(10), "sent", "look"),
    new(4, t0.AddSeconds(11), "script", "wave"),
    new(5, t0.AddSeconds(12), "received", "A lamplighter hums an old fixture tune."),
]);
var t1 = t0.AddDays(1);
var s2 = new HistorySession("eeeeeeee000040008000000000000002", ClientDatabase.CanonicalEndpoint(lanternKey), lantern.Name, "", t1);
history.Append(s2, [new(0, t1, "received", "Rain falls on the fixture road."), new(1, t1.AddSeconds(3), "sent", "north")]);
var s3 = new HistorySession("eeeeeeee000040008000000000000003", ClientDatabase.CanonicalEndpoint(harbor.Host + ":" + harbor.Port), harbor.Name, "Quill", t1, t1.AddMinutes(5));
history.Append(s3, [new(0, t1, "received", "Gulls circle the fixture harbor.")]);
var s4 = new HistorySession("eeeeeeee000040008000000000000004", "demo", "Demo", "", t1, t1.AddMinutes(1));
history.Append(s4, [new(0, t1, "received", "This deleted fixture session must not come back.")]);
history.Delete(s4.Id);

// Fold the WAL into the file so the fixture is one file.
SqliteConnection.ClearAllPools();
using (var connection = new SqliteConnection($"Data Source={dbPath}"))
{
    connection.Open();
    using var checkpoint = connection.CreateCommand();
    checkpoint.CommandText = "PRAGMA wal_checkpoint(TRUNCATE); VACUUM;";
    checkpoint.ExecuteNonQuery();
}
SqliteConnection.ClearAllPools();
foreach (var sidecar in new[] { "-wal", "-shm" }) if (File.Exists(dbPath + sidecar)) File.Delete(dbPath + sidecar);
foreach (var folder in new[] { "maps", "scripts" }) if (Directory.Exists(Path.Combine(output, folder))) Directory.Delete(Path.Combine(output, folder), true);
Console.WriteLine($"wrote {dbPath} ({new FileInfo(dbPath).Length} bytes)");

sealed class FixedClock(DateTimeOffset start) : TimeProvider
{
    public DateTimeOffset Now { get; set; } = start;
    public override DateTimeOffset GetUtcNow() => Now;
}
