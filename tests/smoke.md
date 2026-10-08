# Smoke Test

1. Start `nexora-daemon`.
2. Run `nexora ping` and expect `Pong`.
3. Run `nexora health` and confirm session detection.
4. Run the color command with `#FF8800`.
5. Run the rename planner and verify it only prints a plan; it does not mutate files.
