# Baogram badge web flasher

A local web page that installs the Baogram UF2 image set onto a DC34
badge. The flashing logic is Python running in the browser via
[Pyodide](https://pyodide.org); the page uses the File System Access API
to write the images onto the badge's UF2 bootloader volume.

> **Irreversible:** installing developer-signed images permanently erases
> the badge's factory light-encryption key and converts it to developer
> mode (see BAOGRAM_HARDWARE_TEST.md, warning and §2). The page repeats
> this warning and requires acknowledgement before flashing.

## Usage

1. Build the images: `scripts/build-baogram.sh` (from this repo).
2. `python3 tools/webflash/serve.py` — stages the three UF2s and opens
   `http://127.0.0.1:8342/` in your browser (Chrome/Edge required; the
   File System Access API is not in Safari/Firefox).
3. Tick the acknowledgement box.
4. Put the badge in update mode (README "Updates" section) so it mounts
   as a USB drive, then click the button and pick that drive. The page
   validates the volume (`INFO_UF2.TXT`) and writes `loader.uf2`.
5. The badge reboots and re-mounts; repeat the click for `swap.uf2`,
   then `xous.uf2` — the page walks you through the order.
6. Power-cycle. First boot is slow (PDDB initialization); after that the
   badge starts in the Baogram feed.

## Why one click per image

Web pages cannot write to a newly plugged USB drive on their own:

* WebUSB is blocked for mass-storage interfaces (the OS claims them), so
  a UF2 bootloader cannot be reached that way.
* The File System Access API requires a user gesture per directory
  grant, and the badge re-enumerates as a *new* volume after each image,
  which invalidates the previous grant.

Truly automatic flash-on-plug needs a native helper; for that, copy the
three files by hand or script it with `cp` per BAOGRAM_HARDWARE_TEST.md.

## Testing the UF2 validator

The validation code is plain Python and runs under CPython too:

```
python3 tools/webflash/flash.py \
    ../xous-core/target/riscv32imac-unknown-xous-elf/release/*.uf2
```
