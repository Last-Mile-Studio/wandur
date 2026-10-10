// Each complete server line
mud.on(Events.Line, (event) => {
  if (event.text.startsWith("Obvious exits:")) {
    mud.echo(event.text);
  }
});

// Structured server data (when supplied)
mud.on(Events.Gmcp, (event) => {
  if (event.package === "Char.Vitals" && event.data) {
    mud.echo(JSON.stringify(event.data));
  }
});

mud.alias(/^lh$/, () => mud.send("look"));
mud.trigger(/^Exits: (.+)$/, (match) => mud.echo(match[1]));
// mud.every(60, () => mud.echo("60s"));