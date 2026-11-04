Fonts built into the launcher (`include_bytes!` in `crates/nexo-app/src/screens/settings.rs`).
Both are under the SIL Open Font License 1.1; the licence travels with them here.

- `Monocraft.ttf`, `Monocraft-Bold.ttf`: Monocraft v4.2.1 by Idrees Hassan,
  https://github.com/IdreesInc/Monocraft. `Monocraft-OFL.txt`.
- `StandardGalacticAlphabet.ttf`: "Standard Galactic Alphabet" by Dao Creation God,
  https://github.com/DaoCreationGod/Standard-Galactic-Alphabet-Font (the repo's README
  states OFL; it ships no licence file, so `StandardGalacticAlphabet-OFL.txt` is the
  OFL text under its copyright line). Covers letters and 1-9 only; everything else
  falls back to another font.

Neither is Mojang's font: Minecraft's own glyphs are not ours to ship.
