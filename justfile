# List of all recipes
default:
  @just --list

build:
	RUSTFLAGS=-Awarnings cargo build --release

flamegraph:
	sudo cargo flamegraph  -- src/editor.rs

bloat:
	cargo bloat --crates --release

build-timings:
	cargo build --timings --release

depgraph:
	cargo depgraph | dot -Tpng > graph.png
