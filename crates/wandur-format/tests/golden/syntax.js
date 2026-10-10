// What the parser has to take in a world script.
var count = 0, last;
const pick = ({ name, hp = 0 }, ...rest) => [name, hp, rest.length];
class Watch extends Object { static max = 3; #seen = new Set(); see(n) { if (!this.#seen.has(n)) { this.#seen.add(n); return true } return false } }
const w = new Watch();
mud.trigger(/^(\w+) says, "(.*)"$/i, (m) => { const [, who, what] = m; if (w.see(who)) mud.echo(`${who}: ${what ?? ""}`); });
label: for (let i = 0; i < 3; i++) { switch (i) { case 0: continue label; default: break label } }
try { JSON.parse("{") } catch { count++ } finally { last = count > 0 ? 'failed' : 'ok' }
async function later() { await null; return typeof last === "string" && last?.length >= 2 }
mud.every(60, function () { return later().then(x => x && mud.echo('it\'s "' + last + '"')) });
