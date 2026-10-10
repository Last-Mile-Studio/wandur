// Watch for exits
mud.on(Events.Line, event => {
    if (event.text.startsWith("Exits:") || event.text.startsWith("Obvious exits:")) {
        mud.echo(event.text);
    }
});

mud.alias(/^lh$/, () => mud.send("look"));