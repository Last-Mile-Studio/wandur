// Ferry times at the ford, from the world's directory pack.
const ferry = mud.panel("ferry", { title: "Ferry", dock: "right" });
ferry.label("next", { text: "Next crossing at dusk" });
ferry.button("ask", {
  label: "Ask the ferryman",
  onClick: () => mud.send("ask ferryman crossing"),
});
