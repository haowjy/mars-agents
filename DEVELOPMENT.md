# Development

## Workstation: memory-safe ripgrep

`rg` on this machine is wrapped at `~/.local/bin/rg` to prevent OOM kills. A bare `rg` over a large codebase can mmap hundreds of GB of virtual address space and exhaust RAM (this happened — it killed a tmux session).

The wrapper enforces:
- `--no-mmap` — sequential reads instead of memory-mapped I/O, keeps VSZ low
- `--max-filesize 500M` — skips files larger than 500 MB
- `-j 4` — caps parallel search threads
- cgroup limits via `systemd-run`: `MemoryHigh=6G`, `MemoryMax=10G`, `MemorySwapMax=4G`

To bypass (e.g. searching a legitimately large file):
```bash
~/.local/bin/rg.real [flags] [pattern] [path]
```

Also recommended — install `earlyoom` as a system-wide backstop:
```bash
sudo apt install earlyoom
# In /etc/default/earlyoom:
# EARLYOOM_ARGS="-m 4,3 -s 10,5 --prefer '(^|/)(rg|ripgrep)$' --sort-by-rss -g -r 60"
sudo systemctl enable --now earlyoom
```
