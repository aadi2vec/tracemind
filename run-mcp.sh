#!/bin/bash
export PATH="$HOME/.cargo/bin:$PATH"
exec cargo run -p tm-mcp --release
