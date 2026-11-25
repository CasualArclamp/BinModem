# Build on Linux

[Back to the README](../../README.md)

The releases are Windows executables. CI builds the Linux executable on every
change but does not run it. Thus reports from people who run it keep this page
correct.

The page is for Debian or Ubuntu. Other distributions need the same things
under their own names.

## Sound

Sound goes through the sound server when there is one (PipeWire or
PulseAudio). Thus a device that BinModem uses stays available to all other
programs. `BINMODEM_AUDIO_HOST=alsa` opens the hardware through ALSA instead.
ALSA then gives the device to BinModem alone.

## What it needs

- a Rust toolchain that is sufficiently recent for the 2024 edition (1.85 or
  later)
- a C linker
- the headers of ALSA for the sound card

The window opens on X11 or Wayland through OpenGL, which a desktop
installation already has.

## Procedure

1. Install the packages.

   ```bash
   sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev libclang-dev libxkbcommon0 libgl1
   ```

2. Install the Rust toolchain.

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

3. Open a new terminal, so that `cargo` is on the path.

4. Get the source.

   ```bash
   git clone https://github.com/CasualArclamp/BinModem.git
   ```

   ```bash
   cd BinModem
   ```

5. Build the program.

   ```bash
   cargo build --release -p gui
   ```

   This gives one file, `target/release/binmodem`.

6. Replay the capture that comes with the program. This shows that it works
   with no line and no sound card.

   ```bash
   ./target/release/binmodem --capture
   ```

7. List the audio devices that the machine has.

   ```bash
   ./target/release/binmodem --devices
   ```

8. Open the modem on a line.

   ```bash
   ./target/release/binmodem
   ```

## Other lines

`--telnet` is a board over a socket. `--sip <account>` is a call over a SIP
trunk. These two do not need a sound card. [docs/usage.md](../usage.md) has
every flag.
