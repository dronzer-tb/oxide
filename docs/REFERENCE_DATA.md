# Reference data

Oxide is data-driven (see `ARCHITECTURE.md`): it loads vanilla's exported worldgen registries as
JSON instead of hardcoding noise constants. That JSON has to come from a real server jar, extracted
locally. This is a manual, local step — nothing in this repo downloads it for you.

## Extracting the data

You need the Minecraft 26.2 server jar. Run its built-in data generator:

```sh
java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports
```

`--reports` is enough for `oxide-datapack` (worldgen registries + `generated/reports/version.json`,
which carries the `DataVersion` integer this crate reads at load time — see `ARCHITECTURE.md`'s
"never hardcoded" rule). If you also want block/item/recipe data for other crates, use `--all`
instead:

```sh
java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --all
```

Either command writes output under a `generated/` directory next to wherever you ran it.

## Where Oxide expects it

Copy (or symlink) the generated worldgen tree into `reference/` at the repo root:

```
reference/
  data/<namespace>/worldgen/...
  data/<namespace>/dimension/...
  data/<namespace>/dimension_type/...
  version.json          # from generated/reports/version.json
  pack.mcmeta            # optional
```

`reference/` is gitignored. `oxide-datapack`'s loader (`load_datapack`) takes this directory as its
pack root.

## This data is Mojang-derived — it stays local

Extracted registry JSON and any reference chunk NBT are **never committed or redistributed**. Do not
add `reference/` contents to git, do not attach them to issues/PRs, do not upload them anywhere. This
is a hard rule (PRD §4, §8), not a suggestion — treat any extracted vanilla data the same way you'd
treat a copy of the server jar itself.
