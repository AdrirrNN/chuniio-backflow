# chuniio-backflow

A chuniio.dll implementation that forwards all calls to [Backflow](https://github.com/FyraLabs/backflow)'s chuniio_proxy output backend via Unix domain sockets through Wine.

## Overview

This library implements the chuniio.dll interface expected by CHUNITHM games and forwards all communication to a Unix domain socket exposed by Backflow's chuniio_proxy output backend.

### Architecture

```text
CHUNITHM Game (Windows/Wine) 
    ↓ chuniio.dll API calls
chuniio.dll (this library) 
    ↓ AF_UNIX socket communication
Backflow chuniio_proxy (Linux)
    ↓ Input/Feedback events
Backflow backends (uinput, etc.)
```

This enables CHUNITHM games running under Wine to communicate with native Linux input/output systems through Backflow.

## Building

### Prerequisites

- Rust toolchain with Windows cross-compilation target
- Wine (for testing)

### Build Steps

1. Add the Windows target:

   ```bash
   rustup target add x86_64-pc-windows-gnu
   ```

2. Build the DLL:

   ```bash
   cargo build --target x86_64-pc-windows-gnu --release
   ```

3. The resulting DLL will be at:

   ```text
   target/x86_64-pc-windows-gnu/release/chuniio_backflow.dll
   ```

## Logging

The DLL includes comprehensive logging using the `tracing` crate. Logs are written to stdout/stderr and will appear in the terminal where the game is launched.

### Log Levels

- **DEBUG**: Detailed function call traces and protocol messages
- **INFO**: Initialization and connection status
- **WARN**: Non-fatal errors and warnings
- **ERROR**: Fatal errors and connection failures

### Enabling Logs

Set the `RUST_LOG` environment variable to control logging:

```bash
# Debug level logging for chuniio-backflow
export RUST_LOG=chuniio_backflow=debug

# Info level logging (less verbose)
export RUST_LOG=chuniio_backflow=info

# Error level only
export RUST_LOG=chuniio_backflow=error
```

Then run your game under Wine:

```bash
wine start.bat
```

### Example Log Output

```log
INFO chuniio-backflow: chuniio-backflow DLL loaded
DEBUG chuniio-backflow: Initializing socket connection to chuniio proxy
DEBUG chuniio-backflow: Created Unix domain socket
DEBUG chuniio-backflow: Connecting to socket path: /tmp/chuniio_proxy.sock
INFO chuniio-backflow: Successfully connected to chuniio proxy socket
DEBUG chuniio-backflow: Initializing JVS subsystem
DEBUG chuniio-backflow: JVS subsystem initialized successfully
DEBUG chuniio-backflow: Initializing slider subsystem
DEBUG chuniio-backflow: Starting slider input polling
DEBUG chuniio-backflow: Initializing LED subsystem
DEBUG chuniio-backflow: Updating slider LED colors
DEBUG chuniio-backflow: Message sent (no response expected)
```

## Usage

### 1. Configure Backflow

Add chuniio_proxy backend to your Backflow configuration:

```toml
[output.chuniio_proxy]
enabled = true
socket_path = "/tmp/chuniio_proxy.sock"
```

### 2. Install the DLL

Copy the built DLL to your CHUNITHM game directory and rename it:

```bash
cp target/x86_64-pc-windows-gnu/release/chuniio_backflow.dll /path/to/chunithm/chuniio.dll
```

### 3. Configure Socket Path (Optional)

Set the socket path via environment variable if using a non-default location:

```bash
export CHUNIIO_PROXY_SOCKET="/custom/path/to/chuniio_proxy.sock"
```

### 4. Run the Game

1. Start Backflow with chuniio_proxy enabled
2. Run your CHUNITHM game under Wine
3. The DLL will automatically connect to the socket and forward all chuniio API calls

## Supported Features

This implementation supports all standard chuniio.dll APIs:

### JVS (Input) Functions

- `chuni_io_jvs_init()` - Initialize JVS subsystem
- `chuni_io_jvs_poll()` - Poll operator buttons and IR beams
- `chuni_io_jvs_read_coin_counter()` - Read coin counter

### Slider Functions

- `chuni_io_slider_init()` - Initialize slider subsystem
- `chuni_io_slider_start()` - Start slider polling with callback
- `chuni_io_slider_stop()` - Stop slider polling

### LED Output Functions

- `chuni_io_led_init()` - Initialize LED subsystem
- `chuni_io_slider_set_leds()` - Set slider LED colors
- `chuni_io_led_set_colors()` - Set LED board colors

## Protocol

The DLL communicates with Backflow using a binary protocol over Unix domain sockets:

- **JVS Poll** (0x01) - Request current input state
- **JVS Poll Response** (0x02) - Current operator buttons and IR beams
- **Coin Counter Read** (0x03) - Request coin count
- **Coin Counter Response** (0x04) - Current coin count
- **Slider Input** (0x05) - Slider pressure data
- **Slider LED Update** (0x06) - Update slider LEDs
- **LED Update** (0x07) - Update LED boards
- **Ping** (0x08) / **Pong** (0x09) - Keepalive

## Configuration

### Environment Variables

- `CHUNIIO_PROXY_SOCKET` - Override socket path (default: `/tmp/chuniio_proxy.sock`)

### Backflow Input Mapping

The chuniio_proxy backend recognizes these special input keycodes:

- `CHUNIIO_COIN` - Coin insertion
- `CHUNIIO_TEST` - Test button
- `CHUNIIO_SERVICE` - Service button  
- `CHUNIIO_SLIDER_[0-31]` - Slider touch zones
- `CHUNIIO_IR_[0-5]` - IR beam sensors

## Troubleshooting

### Connection Issues

- Ensure Backflow is running with chuniio_proxy enabled
- Check that the socket path is correct
- Verify Wine can access Unix sockets (Wine 6.0+ recommended)

### Permission Issues

- Make sure the socket file has appropriate permissions
- Check that Wine process can write to the socket directory

### LED/Feedback Issues

- Verify Backflow feedback configuration
- Check that LED events are being processed by Backflow

## License

GPL-3.0-or-later
