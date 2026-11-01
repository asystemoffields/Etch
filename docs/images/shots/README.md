# README screenshots

Each `.jsonl` file is a script for the headless screenshot tool. From the repository root:

```sh
cargo run --release -p wordcraft-ui-egui --example ui_shot -- docs/images/shots/hero.jsonl
cargo run --release -p wordcraft-ui-egui --example ui_shot -- docs/images/shots/dark.jsonl
```

Each script opens the built-in sample document in a 1440×900 window at 2× and writes the PNG named in
its `shot` line, relative to the repository root. Rerun them after a visible interface change.
