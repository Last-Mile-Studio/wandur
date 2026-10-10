// The schema 9 fixture (the feature/mudlet-import branch): one synthetic world whose library holds
// a Lua script with the Mudlet layer and a script imported from a Mudlet profile. Nothing is real.
//
// Usage: dotnet run --project tools/FixtureGen -- <output directory>
using Microsoft.Data.Sqlite;
using Wandur.Core.Scripting;
using Wandur.Core.Settings;
using Wandur.Core.Storage;

var output = Path.GetFullPath(args.Length > 0 ? args[0] : "csharp-data-v9");
if (Directory.Exists(output)) Directory.Delete(output, recursive: true);
Directory.CreateDirectory(output);
var dbPath = Path.Combine(output, "wandur.db");
var database = new ClientDatabase(dbPath);
var settings = new SqliteSettingsStore(database, Path.Combine(output, "settings.json"));
var world = new ConnectionProfile
{
    Id = Guid.Parse("11111111-2222-4333-8444-555555555591"), Name = "Fixture Reed Marsh", Host = "marsh.fixture.example", Port = 5555,
};
settings.Save(new ClientSettings { Profiles = [world] });
var scripts = new SqliteWorldScriptLibraryStore(database, Path.Combine(output, "scripts"));
var key = world.Host + ":" + world.Port;
scripts.Load(key);
scripts.Upsert(key, new(Guid.Parse("cccccccc-0000-4000-8000-000000000091"), "Fixture Lua greeter",
    "send(\"wave\")", Enabled: true) { Language = ScriptLanguages.Lua, Compatibility = ScriptCompatibility.Mudlet });
var source = "mud.send('look');";
scripts.Upsert(key, new(Guid.Parse("cccccccc-0000-4000-8000-000000000092"), "Fixture imported", source, Enabled: false)
{
    Import = new ScriptImportInfo(ScriptImportInfo.Mudlet, "fixture.xml", "Fixture group", false,
        [new ImportedItem("trigger", "Fixture group/Fixture trigger", "Fixture trigger", "", "send(\"look\")", ["^fixture$"])])
    { SourceHash = ScriptImportInfo.Hash(source) },
});
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
if (Directory.Exists(Path.Combine(output, "scripts"))) Directory.Delete(Path.Combine(output, "scripts"), true);
Console.WriteLine($"wrote {dbPath} ({new FileInfo(dbPath).Length} bytes)");
