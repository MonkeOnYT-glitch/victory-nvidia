# Archived branch

This branch is the earlier attempt on Belobug's VIctory-Belobug-AMD fork:
the hang-watchdog patch, an NVIDIA fix for the frame-rate readout, the PC
FidelityFX hand-off, and diagnostics. On an RTX 5060 Ti the Series X profile
reached the world (about 6 fps, broken picture); the Series S profile lost the
device at its first world frame, cause not found.

**The current work is on `main`**: Vice-Tay (TaynanGT/Vice-Tay, built on this
fork) with changes for an 8 GB NVIDIA card. Series S runs there at about
16.5 fps, ray tracing at about 13.5 fps. See `main`'s README.
