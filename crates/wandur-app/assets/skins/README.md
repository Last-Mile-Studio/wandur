# Skin assets

Both images come from the C# client (`wandur-client`, same owner, MIT), at 27f66d2:

- `armored-wear-384.png`: `src/Wandur.Desktop/Assets/Skins/armored-wear.png`, scaled from 1254 to
  384 pixels square with `sips` (the C# client tiles it at 384 DIP). It is an original generated
  transparent PNG made for the C# client on 2026-09-25 (see that folder's README for its prompt).
  Only its alpha channel is used: a faint dark scratch and an offset light lip, clipped to the
  Armored skin's metal plates, so it works on any palette.
- `app-icon-64.png`: `src/Wandur.Desktop/Assets/icon-256.png` scaled to 64 pixels, the icon on
  the title plate and in the System skin's toolbar.
