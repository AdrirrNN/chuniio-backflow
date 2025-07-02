//! chuniio-backflow: A chuniio.dll implementation that forwards all calls to Backflow's chuniio_proxy
//!
//! This library implements the chuniio.dll interface expected by CHUNITHM games
//! and forwards all communication to a Unix domain socket exposed by Backflow's
//! chuniio_proxy output backend.
//!
//! ## Architecture
//!
//! This DLL acts as a bridge between the Windows CHUNITHM game and the Linux
//! Backflow backend running in Wine:
//!
//! ```text
//! CHUNITHM Game (Windows/Wine) -> chuniio.dll (this library) -> AF_UNIX socket -> Backflow chuniio_proxy (Linux)
//! ```
//!
//! ## Usage
//!
//! 1. Build this library as a Windows DLL using `cargo build --target x86_64-pc-windows-gnu --release`
//! 2. Copy the resulting `chuniio_backflow.dll` to the game directory as `chuniio.dll`
//! 3. Configure Backflow with the chuniio_proxy backend enabled
//! 4. Run the game under Wine
//!
//! The DLL will automatically connect to the socket at `/tmp/chuniio_proxy.sock` (configurable via environment)

#![allow(clippy::missing_safety_doc)]

use std::{
    ffi::{c_void, CString},
    mem,
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use tracing::{debug, error, info, warn};

use winapi::{
    shared::{
        minwindef::{BOOL, DWORD, HINSTANCE, LPVOID, TRUE},
        winerror::{E_FAIL, S_OK},
    },
    um::{
        processenv::GetEnvironmentVariableA,
        winnt::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH, HRESULT},
    },
};

use windows::Win32::Networking::WinSock::{
    closesocket, connect, recv, send, socket, WSACleanup, WSAStartup, AF_UNIX, SEND_RECV_FLAGS,
    SOCKADDR, SOCKET, SOCKET_ERROR, SOCK_STREAM, WSADATA,
};

mod protocol;
use protocol::*;

/// Default socket path for chuniio proxy
const DEFAULT_SOCKET_PATH: &str = "/tmp/chuniio_proxy.sock";

/// Environment variable for socket path override
const SOCKET_PATH_ENV: &str = "CHUNIIO_PROXY_SOCKET";

/// Global state for the DLL
struct GlobalState {
    /// Socket connection to chuniio proxy
    socket: Option<SOCKET>,
    /// Current JVS state (operator buttons and IR beams)
    jvs_state: JvsState,
    /// Coin counter
    coin_counter: AtomicU16,
    /// Whether the slider is active
    slider_active: AtomicBool,
    /// Slider callback function
    slider_callback: Option<SliderCallbackFn>,
    /// Current slider pressure data
    slider_pressure: [u8; 32],
    /// LED subsystem initialization state
    led_initialized: bool,
    /// LED board states for each board (0=billboard left, 1=billboard right, 2=slider)
    led_board_states: [Vec<u8>; 3],
}

#[derive(Default)]
struct JvsState {
    opbtn: u8, // operator button bits
    beams: u8, // IR beam bits
}

type SliderCallbackFn = unsafe extern "C" fn(data: *const u8);

static GLOBAL_STATE: Mutex<GlobalState> = Mutex::new(GlobalState {
    socket: None,
    jvs_state: JvsState { opbtn: 0, beams: 0 },
    coin_counter: AtomicU16::new(0),
    slider_active: AtomicBool::new(false),
    slider_callback: None,
    slider_pressure: [0; 32],
    led_initialized: false,
    led_board_states: [Vec::new(), Vec::new(), Vec::new()],
});

// Guard to keep the file appender alive
static mut _LOG_GUARD: Option<tracing_appender::non_blocking::WorkerGuard> = None;

/// Initialize Winsock and connect to the chuniio proxy socket
unsafe fn init_socket_connection() -> Option<SOCKET> {
    debug!("Initializing socket connection to chuniio proxy");

    // Initialize Winsock
    let mut wsadata: WSADATA = mem::zeroed();
    if WSAStartup(0x0202, &mut wsadata) != 0 {
        error!("Failed to initialize Winsock");
        return None;
    }

    // Create Unix domain socket
    let sock = match socket(AF_UNIX.into(), SOCK_STREAM, 0) {
        Ok(s) => {
            debug!("Created Unix domain socket");
            s
        }
        Err(e) => {
            error!("Failed to create socket: {:?}", e);
            WSACleanup();
            return None;
        }
    };

    // Get socket path from environment or use default
    let socket_path = get_socket_path();
    debug!("Connecting to socket path: {}", socket_path);
    let socket_path_cstring = CString::new(socket_path).ok()?;

    // Create sockaddr_un structure for Unix socket
    let mut addr: [u8; 110] = [0; 110]; // sockaddr_un size
    addr[0] = AF_UNIX as u8; // sa_family
    addr[1] = 0;

    // Copy the path starting at offset 2
    let path_bytes = socket_path_cstring.as_bytes();
    for (i, &byte) in path_bytes.iter().enumerate() {
        if i + 2 < addr.len() {
            addr[i + 2] = byte;
        }
    }

    // Connect to the Unix socket
    if connect(sock, addr.as_ptr() as *const SOCKADDR, addr.len() as i32) == SOCKET_ERROR {
        error!("Failed to connect to chuniio proxy socket");
        closesocket(sock);
        WSACleanup();
        return None;
    }

    info!("Successfully connected to chuniio proxy socket");
    Some(sock)
}

/// Get socket path from environment variable or use default
fn get_socket_path() -> String {
    unsafe {
        let mut buffer = [0u8; 260]; // MAX_PATH
        let env_var = CString::new(SOCKET_PATH_ENV).unwrap();
        let len = GetEnvironmentVariableA(
            env_var.as_ptr(),
            buffer.as_mut_ptr() as *mut i8,
            buffer.len() as u32,
        );

        if len > 0 && len < buffer.len() as u32 {
            if let Ok(path) = CString::new(&buffer[..len as usize]) {
                if let Ok(path_str) = path.to_str() {
                    return path_str.to_string();
                }
            }
        }
    }

    DEFAULT_SOCKET_PATH.to_string()
}

/// Attempt to recover socket connection if lost
unsafe fn recover_connection() -> bool {
    debug!("Attempting to recover socket connection");

    if let Some(new_sock) = init_socket_connection() {
        if let Ok(mut state) = GLOBAL_STATE.lock() {
            // Close old socket if it exists
            if let Some(old_sock) = state.socket.take() {
                closesocket(old_sock);
            }

            state.socket = Some(new_sock);
            info!("Socket connection recovered successfully");
            return true;
        }
    }

    warn!("Failed to recover socket connection");
    false
}

/// Send a message with automatic connection recovery
unsafe fn send_message_with_recovery(message: &ChuniMessage) -> Option<ChuniMessage> {
    if let Ok(state) = GLOBAL_STATE.lock() {
        if let Some(sock) = state.socket {
            // Try to send with existing connection
            let result = send_message(sock, message);
            if result.is_some() {
                return result;
            }
        }
    }

    // If we get here, either no connection or send failed
    // Try to recover connection and retry once
    if recover_connection() {
        if let Ok(state) = GLOBAL_STATE.lock() {
            if let Some(sock) = state.socket {
                debug!("Retrying message send after connection recovery");
                return send_message(sock, message);
            }
        }
    }

    None
}

/// Send a message to the chuniio proxy and optionally receive a response
unsafe fn send_message(sock: SOCKET, message: &ChuniMessage) -> Option<ChuniMessage> {
    // Serialize message
    let data = message.serialize();

    // Only log detailed info for non-polling messages to reduce noise
    match message {
        ChuniMessage::JvsPoll | ChuniMessage::CoinCounterRead | ChuniMessage::SliderStateRead => {
            // Silent for frequent operations
        }
        _ => {
            debug!("Sending message: {:?} ({} bytes)", message, data.len());
        }
    }

    // Send message
    if send(sock, &data, SEND_RECV_FLAGS(0)) == SOCKET_ERROR {
        match message {
            ChuniMessage::JvsPoll
            | ChuniMessage::CoinCounterRead
            | ChuniMessage::SliderStateRead => {
                // Silent for frequent operations
            }
            _ => {
                error!("Failed to send message to chuniio proxy");
            }
        }
        return None;
    }

    // For messages that expect a response, try to receive it
    match message {
        ChuniMessage::JvsPoll
        | ChuniMessage::CoinCounterRead
        | ChuniMessage::SliderStateRead
        | ChuniMessage::Ping => {
            let mut buffer = [0u8; 1024];
            let bytes_received = recv(sock, &mut buffer, SEND_RECV_FLAGS(0));

            if bytes_received > 0 {
                match ChuniMessage::deserialize(&buffer[..bytes_received as usize]) {
                    Ok(response) => {
                        // Only log non-polling responses to reduce noise
                        match response {
                            ChuniMessage::JvsPollResponse { .. }
                            | ChuniMessage::CoinCounterReadResponse { .. }
                            | ChuniMessage::SliderStateReadResponse { .. }
                            | ChuniMessage::Pong => {
                                // Silent for frequent operations
                            }
                            _ => {
                                debug!("Received response from chuniio proxy: {:?}", response);
                            }
                        }
                        Some(response)
                    }
                    Err(e) => {
                        error!("Failed to deserialize response: {:?}", e);
                        None
                    }
                }
            } else {
                match message {
                    ChuniMessage::JvsPoll
                    | ChuniMessage::CoinCounterRead
                    | ChuniMessage::SliderStateRead => {
                        // Silent for frequent operations
                    }
                    _ => {
                        error!(
                            "Failed to receive response from chuniio proxy (received {} bytes)",
                            bytes_received
                        );
                    }
                }
                None
            }
        }
        _ => {
            debug!("Message sent (no response expected)");
            None // No response expected
        }
    }
}

/// Send a message to the chuniio proxy without waiting for a response (fire-and-forget)
/// This is similar to how the reference implementation sends to named pipes
unsafe fn send_message_fire_and_forget(message: &ChuniMessage) {
    if let Ok(state) = GLOBAL_STATE.lock() {
        if let Some(sock) = state.socket {
            // Serialize message
            let data = message.serialize();

            // Send message without waiting for response
            if send(sock, &data, SEND_RECV_FLAGS(0)) == SOCKET_ERROR {
                // Silently fail like reference implementation does with broken pipes
            }
        }
    }
}

// ============================================================================
// DLL Entry Point
// ============================================================================

#[cfg_attr(target_os = "windows", export_name = "DllMain")]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllMain(
    _hinst_dll: HINSTANCE,
    fdw_reason: DWORD,
    _lpv_reserved: LPVOID,
) -> BOOL {
    match fdw_reason {
        x if x == DLL_PROCESS_ATTACH => {
            // Create log file appender in current directory
            let file_appender = tracing_appender::rolling::never(".", "chuniio-backflow.log");
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

            // Store the guard to keep the appender alive
            _LOG_GUARD = Some(guard);

            // Create an env filter that defaults to "trace" level if RUST_LOG is not set
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("trace"));

            // Initialize tracing subscriber for logging to file
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_target(false)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .with_writer(non_blocking)
                .try_init();

            info!("chuniio-backflow DLL loaded");

            // Initialize connection to chuniio proxy
            if let Some(sock) = init_socket_connection() {
                if let Ok(mut state) = GLOBAL_STATE.lock() {
                    state.socket = Some(sock);
                    info!("Successfully connected to chuniio proxy");

                    // Test the connection with a ping
                    debug!("Testing connection with ping...");
                    let ping_message = ChuniMessage::Ping;
                    if let Some(response) = send_message(sock, &ping_message) {
                        info!("Ping test successful: {:?}", response);
                    } else {
                        error!("Ping test failed - connection may be unstable");
                    }
                } else {
                    error!("Failed to acquire global state lock");
                }
            } else {
                warn!("Failed to connect to chuniio proxy - will retry on API calls");
            }
        }
        x if x == DLL_PROCESS_DETACH => {
            // Cleanup
            if let Ok(mut state) = GLOBAL_STATE.lock() {
                if let Some(sock) = state.socket.take() {
                    closesocket(sock);
                    WSACleanup();
                }
            }
        }
        _ => {}
    }
    TRUE
}

// ============================================================================
// JVS (Input) Functions
// ============================================================================

/// Initialize JVS subsystem
#[no_mangle]
pub unsafe extern "C" fn chuni_io_jvs_init() -> HRESULT {
    debug!("chuni_io_jvs_init called - starting JVS initialization");

    // Connection should already be established in DllMain
    if let Ok(state) = GLOBAL_STATE.lock() {
        if state.socket.is_some() {
            debug!("JVS subsystem initialized successfully");

            // Test connectivity immediately after init
            debug!("Testing immediate JVS poll after init...");
            if let Some(sock) = state.socket {
                let test_message = ChuniMessage::JvsPoll;
                if let Some(response) = send_message(sock, &test_message) {
                    info!("Immediate JVS poll test successful: {:?}", response);
                } else {
                    error!("Immediate JVS poll test failed");
                }
            }

            // Note: In the reference implementation, JVS init also creates the LED mutex
            // Since we don't use Windows mutexes, we'll handle LED synchronization in Rust
            debug!("LED synchronization mutex equivalent created");

            info!("JVS and LED synchronization initialized");
            return S_OK;
        } else {
            error!("JVS init failed: no socket connection");
            return E_FAIL;
        }
    } else {
        error!("JVS init failed: could not acquire global state lock");
        return E_FAIL;
    }
}

/// Poll JVS inputs (operator buttons and IR beams)
#[no_mangle]
pub unsafe extern "C" fn chuni_io_jvs_poll(opbtn: *mut u8, beams: *mut u8) {
    if opbtn.is_null() || beams.is_null() {
        warn!("chuni_io_jvs_poll called with null pointers");
        return;
    }

    // First, return current cached state
    if let Ok(state) = GLOBAL_STATE.try_lock() {
        *opbtn = state.jvs_state.opbtn;
        *beams = state.jvs_state.beams;

        // If we have a connection, try to update state quickly
        if state.socket.is_some() {
            drop(state); // Release lock before socket operation

            // Send JVS poll request with very short timeout
            let message = ChuniMessage::JvsPoll;
            if let Some(response) = send_message_with_recovery(&message) {
                if let ChuniMessage::JvsPollResponse {
                    opbtn: op,
                    beams: ir,
                } = response
                {
                    // Update state for next call
                    if let Ok(mut state) = GLOBAL_STATE.try_lock() {
                        state.jvs_state.opbtn = op;
                        state.jvs_state.beams = ir;

                        // Return the updated state
                        *opbtn = op;
                        *beams = ir;
                    }
                }
            }
        }
    } else {
        // If we can't get lock immediately, return empty state
        *opbtn = 0;
        *beams = 0;
    }
}

/// Read coin counter
#[no_mangle]
pub unsafe extern "C" fn chuni_io_jvs_read_coin_counter(total: *mut u16) {
    if total.is_null() {
        warn!("chuni_io_jvs_read_coin_counter called with null pointer");
        return;
    }

    // First, return current cached coin count
    if let Ok(state) = GLOBAL_STATE.try_lock() {
        let current_count = state.coin_counter.load(Ordering::Relaxed);
        *total = current_count;

        // If we have a connection, try to update count quickly
        if state.socket.is_some() {
            drop(state); // Release lock before socket operation

            // Send coin counter read request
            let message = ChuniMessage::CoinCounterRead;
            if let Some(response) = send_message_with_recovery(&message) {
                if let ChuniMessage::CoinCounterReadResponse { count } = response {
                    if let Ok(state) = GLOBAL_STATE.try_lock() {
                        state.coin_counter.store(count, Ordering::Relaxed);
                        *total = count; // Return the updated count
                    }
                }
            }
        }
    } else {
        // If we can't get lock immediately, return 0
        *total = 0;
    }
}

// ============================================================================
// Slider Functions
// ============================================================================

/// Initialize slider subsystem
#[no_mangle]
pub unsafe extern "C" fn chuni_io_slider_init() -> HRESULT {
    debug!("chuni_io_slider_init called");

    // In the reference implementation, slider_init calls led_output_init because of slider LEDs
    // We'll ensure LED subsystem is initialized here too
    if let Ok(mut state) = GLOBAL_STATE.lock() {
        if !state.led_initialized {
            debug!("LED subsystem not yet initialized, initializing now for slider LEDs");

            // Initialize LED board state buffers with correct sizes
            state.led_board_states[0] = vec![0u8; 159];
            state.led_board_states[1] = vec![0u8; 189];
            state.led_board_states[2] = vec![0u8; 93];

            state.led_initialized = true;
            debug!("LED subsystem initialized via slider init");
        }

        info!("Slider subsystem initialized successfully");
        return S_OK;
    } else {
        error!("Slider init failed: could not acquire global state lock");
        return E_FAIL;
    }
}

/// Start slider input polling with callback
#[no_mangle]
pub unsafe extern "C" fn chuni_io_slider_start(callback: *const c_void) {
    debug!("chuni_io_slider_start called with callback: {:?}", callback);

    if callback.is_null() {
        warn!("Slider start called with null callback");
        return;
    }

    debug!("Starting slider input polling");

    let callback_fn = std::mem::transmute::<_, SliderCallbackFn>(callback);

    if let Ok(mut state) = GLOBAL_STATE.lock() {
        if state.slider_active.load(Ordering::SeqCst) {
            debug!("Slider already active, returning");
            return; // Already running
        }

        state.slider_callback = Some(callback_fn);
        state.slider_active.store(true, Ordering::SeqCst);

        let _sock = state.socket;
        drop(state); // Release lock before spawning thread

        // Spawn slider polling thread
        thread::spawn(move || {
            debug!("Slider polling thread started");
            while GLOBAL_STATE
                .lock()
                .map(|s| s.slider_active.load(Ordering::SeqCst))
                .unwrap_or(false)
            {
                // Query proxy for current slider state and call callback with it
                if let Ok(state) = GLOBAL_STATE.lock() {
                    // Try to get updated slider data from proxy
                    if let Some(_sock) = state.socket {
                        drop(state); // Release lock before socket operation

                        // Send slider state read request to get current state
                        let request = ChuniMessage::SliderStateRead;
                        if let Some(response) = send_message_with_recovery(&request) {
                            if let ChuniMessage::SliderStateReadResponse { pressure } = response {
                                // Update cached state with response from proxy
                                if let Ok(mut state) = GLOBAL_STATE.lock() {
                                    state.slider_pressure = pressure;
                                    // Call callback with updated data
                                    if let Some(callback) = state.slider_callback {
                                        callback(state.slider_pressure.as_ptr());
                                    }
                                }
                            } else {
                                // Unexpected response, use cached data
                                if let Ok(state) = GLOBAL_STATE.lock() {
                                    if let Some(callback) = state.slider_callback {
                                        callback(state.slider_pressure.as_ptr());
                                    }
                                }
                            }
                        } else {
                            // No response from proxy, use cached data
                            if let Ok(state) = GLOBAL_STATE.lock() {
                                if let Some(callback) = state.slider_callback {
                                    callback(state.slider_pressure.as_ptr());
                                }
                            }
                        }
                    } else {
                        // No connection, call callback with empty state
                        if let Some(callback) = state.slider_callback {
                            callback(state.slider_pressure.as_ptr());
                        }
                    }
                }

                thread::sleep(Duration::from_millis(1)); // ~1000Hz polling rate
            }
            debug!("Slider polling thread stopped");
        });
    }
}

/// Stop slider input polling
#[no_mangle]
pub unsafe extern "C" fn chuni_io_slider_stop() {
    debug!("chuni_io_slider_stop called");
    if let Ok(state) = GLOBAL_STATE.lock() {
        state.slider_active.store(false, Ordering::SeqCst);
    }
}

// ============================================================================
// LED Output Functions
// ============================================================================

/// Initialize LED subsystem
/// Initialize LED subsystem
#[no_mangle]
pub unsafe extern "C" fn chuni_io_led_init() -> HRESULT {
    if let Ok(mut state) = GLOBAL_STATE.try_lock() {
        if state.led_initialized {
            return S_OK;
        }

        // Initialize LED board state buffers with correct sizes
        // Board 0: 53 LEDs * 3 bytes = 159 bytes (billboard left)
        // Board 1: 63 LEDs * 3 bytes = 189 bytes (billboard right)
        // Board 2: 31 LEDs * 3 bytes = 93 bytes (slider)
        state.led_board_states[0] = vec![0u8; 159];
        state.led_board_states[1] = vec![0u8; 189];
        state.led_board_states[2] = vec![0u8; 93];

        state.led_initialized = true;
        info!("LED boards initialized successfully");
        return S_OK;
    } else {
        warn!(
            "LED init: could not acquire global state lock immediately, returning success anyway"
        );
        return S_OK; // Return success like reference implementation does
    }
}

/// Set slider LED colors
#[no_mangle]
pub unsafe extern "C" fn chuni_io_slider_set_leds(rgb: *const u8) {
    if rgb.is_null() {
        return;
    }

    // In the reference implementation, this calls led_output_update(2, rgb)
    // So we forward to our LED board function for board 2 (slider)
    chuni_io_led_set_colors(2, rgb);
}

/// Set LED board colors
#[no_mangle]
pub unsafe extern "C" fn chuni_io_led_set_colors(board: u8, rgb: *const u8) {
    // Validate parameters like the reference implementation
    if rgb.is_null() {
        return;
    }

    if board > 2 {
        return;
    }

    // Try to acquire lock with timeout to avoid blocking game thread
    if let Ok(mut state) = GLOBAL_STATE.try_lock() {
        // Ensure LED subsystem is initialized
        if !state.led_initialized {
            return;
        }

        // Get correct RGB data size based on board
        let rgb_len = match board {
            0 => 159,    // Board 0: 53 LEDs * 3 bytes = 159 bytes (billboard left)
            1 => 189,    // Board 1: 63 LEDs * 3 bytes = 189 bytes (billboard right)
            2 => 93,     // Board 2: 31 LEDs * 3 bytes = 93 bytes (slider)
            _ => return, // Already validated above
        };

        // Copy RGB data to our internal buffer (like the reference implementation does)
        let rgb_data = std::slice::from_raw_parts(rgb, rgb_len).to_vec();
        state.led_board_states[board as usize] = rgb_data.clone();

        // Send LED data to proxy (like reference sends to named pipe)
        if state.socket.is_some() {
            let message = ChuniMessage::LedUpdate { board, rgb_data };

            // Drop the lock before sending to avoid deadlock
            drop(state);

            // Send asynchronously without waiting for response (fire-and-forget like named pipe)
            std::thread::spawn(move || {
                unsafe { send_message_fire_and_forget(&message) };
            });
        }
    }
    // If we can't get the lock immediately, just silently fail like the reference does

    // Always return immediately, like the reference implementation
}

// ============================================================================
// API Version Function
// ============================================================================

/// Get API version - required by chunithm games to determine compatibility
#[no_mangle]
pub extern "C" fn chuni_io_get_api_version() -> u16 {
    debug!("Reported chuniio API version: 1.2 (LED boards supported)");
    0x0102
}

// ============================================================================
