// Lantern watch: oil and the road ahead, beside the transcript.
const watch = mud.panel("lantern-watch", { title: "Lantern watch", dock: "right" });
const strip = mud.panel("lantern-oil", { dock: "bars" });
let oil = 8;

function draw() {
  watch.gauge("oil", { label: "Lantern oil", value: oil, max: 10, warn: 0.3 });
  watch.gauge("wick", { label: "Wick", value: 64, max: 100 });
  watch.label("road", { text: "&YRoad clear to the ford&D" });
  strip.gauge("oil", { label: "Lantern oil", value: oil, max: 10 });
}

draw();
watch.button("refill", { label: "Refill lantern", onClick: () => mud.send("fill lantern") });
mud.trigger(/^Your lantern gutters/, () => {
  oil = Math.max(0, oil - 1);
  draw();
});
