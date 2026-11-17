# Hourly Naso Phase Checker

This system automatically checks the Naso project's progress every hour, verifies the current phase status, continues work where it left off, checks for AI slop, and pushes changes to GitHub.

## Setup

The hourly check runs via cron at minute 0 of every hour:

```
0 * * * * /home/node/naso/hourly-phase-check.sh >> /home/node/naso/.phase-checker-logs/cron.log 2>&1
```

## Components

### `hourly-phase-check.sh`
Main script that:
1. Reads `naso-progress.md` to determine current active phase
2. Checks phase status (COMPLETED, IN PROGRESS, PENDING)
3. Runs verification tests (`cargo test`)
4. Checks for AI slop patterns in progress file and recent commits
5. Continues work on the current phase if IN PROGRESS
6. Commits and pushes any changes to GitHub
7. Logs all activity to timestamped log files in `.phase-checker-logs/`

### Log Files
- **Per-run logs**: `.phase-checker-logs/phase-checker-YYYYMMDD-HHMMSS.log`
- **Cron aggregate log**: `.phase-checker-logs/cron.log`
- **Retention**: Keeps last 168 hourly logs (1 week)

## AI Slop Detection

The script checks for common AI-generated text patterns:
- Flowery language: "delve", "tapestry", "unwavering", "crucial", "paramount", "unleash", "unlock", "embark", "journey", "realm", "landscape", "holistic", "synergy", "paradigm", "revolutionary", "groundbreaking", "transformative"
- Redundant prefixes: "comprehensive analysis", "in-depth analysis", "thorough analysis"
- Unnecessary hedging: "it is important to note", "please note that"

## Verification

The script runs:
```bash
cargo test -p naso-quantum-runtime -p naso-gates -p async-runtime --lib
```

## GitHub Integration

- Uses `gh` CLI (pre-authenticated)
- Pushes to `naso-lang/naso` repo
- Commits with message: `chore: hourly phase check and continue - YYYY-MM-DD HH:MM`

## Manual Run

```bash
cd /home/node/naso
./hourly-phase-check.sh
```

## View Logs

```bash
# Latest run log
ls -t .phase-checker-logs/phase-checker-*.log | head -1 | xargs cat

# Cron aggregate
tail -f .phase-checker-logs/cron.log
```

## Phase Status Detection

The script reads `naso-progress.md` and looks for:
- Phase headers: `## Phase N` or `## 🎯 Phase N`
- Status markers: `✅`, `🔄`, `⏳`, `IN PROGRESS`, `COMPLETED`, `PENDING`
- Status field: `Status: COMPLETED`

## Current Active Phases

Based on `naso-progress.md`:
- **Phase 13B**: Core Optimization (30,000 req/s) - IN PROGRESS
- **Phase 14**: Core Language Completeness - IN PROGRESS (GATs, Const Generics)
- **Phase 15**: Quantum-First Systems Programming - IN PROGRESS

## Cron Management

```bash
# View current crontab
crontab -l

# Remove cron job
crontab -r

# Edit crontab
crontab -e
```