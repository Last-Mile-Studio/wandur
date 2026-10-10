// Ferry times at the ford, from the world's directory pack.
const ferry = mud.panel("ferry", { title: "Ferry", dock: "right" });
let times = [],
  next = null;
function draw() {
  ferry.label("next", { text: next ? "Next crossing at " + next : "No crossing known" });
  ferry.gauge("wait", { label: "Wait", value: times.length, max: 10, warn: 0.3 });
}
mud.trigger(/^The ferryman calls: next crossing at (\w+)\.$/, (m) => {
  next = m[1];
  times.push(m[1]);
  if (times.length > 10) {
    times.shift();
  }
  draw();
});
mud.on(Events.Gmcp, (e) => {
  if (e.package === "Room.Info" && e.data && e.data.name === "The Ford") {
    mud.send("ask ferryman crossing");
  }
});
ferry.button("ask", {
  label: "Ask the ferryman",
  onClick: () => mud.send("ask ferryman crossing"),
});
draw();
