//! chuniio protocol message definitions for communication with Backflow
//!
//! This module defines the binary protocol messages used to communicate
//! with Backflow's chuniio_proxy backend over Unix domain sockets.

use std::io::{self, Cursor, Read};

/// chuniio protocol message types
#[derive(Debug, Clone)]
pub enum ChuniMessage {
    /// JVS input poll request
    JvsPoll,
    /// JVS input poll response
    JvsPollResponse { opbtn: u8, beams: u8 },
    /// Coin counter read request
    CoinCounterRead,
    /// Coin counter read response
    CoinCounterReadResponse { count: u16 },
    /// Slider pressure input data (32 bytes)
    SliderInput { pressure: [u8; 32] },
    /// Slider state read request
    SliderStateRead,
    /// Slider state read response
    SliderStateReadResponse { pressure: [u8; 32] },
    /// Slider LED update (RGB data)
    SliderLedUpdate { rgb_data: Vec<u8> },
    /// LED board update
    LedUpdate { board: u8, rgb_data: Vec<u8> },
    /// Keepalive ping
    Ping,
    /// Keepalive pong response
    Pong,
    /// JVS full state read request
    JvsFullStateRead,
    /// JVS full state read response
    JvsFullStateReadResponse {
        opbtn: u8,
        beams: u8,
        pressure: [u8; 32],
        coin_counter: u16,
    },
}

/// Message type IDs
impl ChuniMessage {
    pub const JVS_POLL: u8 = 0x01;
    pub const JVS_POLL_RESPONSE: u8 = 0x02;
    pub const COIN_COUNTER_READ: u8 = 0x03;
    pub const COIN_COUNTER_READ_RESPONSE: u8 = 0x04;
    pub const SLIDER_INPUT: u8 = 0x05;
    pub const SLIDER_STATE_READ: u8 = 0x0A;
    pub const SLIDER_STATE_READ_RESPONSE: u8 = 0x0B;
    pub const SLIDER_LED_UPDATE: u8 = 0x06;
    pub const LED_UPDATE: u8 = 0x07;
    pub const PING: u8 = 0x08;
    pub const PONG: u8 = 0x09;
    pub const JVS_FULL_STATE_READ: u8 = 0x0C;
    pub const JVS_FULL_STATE_READ_RESPONSE: u8 = 0x0D;

    /// Serialize message to bytes
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::new();

        match self {
            ChuniMessage::JvsPoll => {
                data.push(Self::JVS_POLL);
            }
            ChuniMessage::JvsPollResponse { opbtn, beams } => {
                data.push(Self::JVS_POLL_RESPONSE);
                data.push(*opbtn);
                data.push(*beams);
            }
            ChuniMessage::CoinCounterRead => {
                data.push(Self::COIN_COUNTER_READ);
            }
            ChuniMessage::CoinCounterReadResponse { count } => {
                data.push(Self::COIN_COUNTER_READ_RESPONSE);
                data.extend_from_slice(&count.to_le_bytes());
            }
            ChuniMessage::SliderInput { pressure } => {
                data.push(Self::SLIDER_INPUT);
                data.extend_from_slice(pressure);
            }
            ChuniMessage::SliderStateRead => {
                data.push(Self::SLIDER_STATE_READ);
            }
            ChuniMessage::SliderStateReadResponse { pressure } => {
                data.push(Self::SLIDER_STATE_READ_RESPONSE);
                data.extend_from_slice(pressure);
            }
            ChuniMessage::SliderLedUpdate { rgb_data } => {
                data.push(Self::SLIDER_LED_UPDATE);
                data.push(rgb_data.len() as u8);
                data.extend_from_slice(rgb_data);
            }
            ChuniMessage::LedUpdate { board, rgb_data } => {
                data.push(Self::LED_UPDATE);
                data.push(*board);
                data.push(rgb_data.len() as u8);
                data.extend_from_slice(rgb_data);
            }
            ChuniMessage::Ping => {
                data.push(Self::PING);
            }
            ChuniMessage::Pong => {
                data.push(Self::PONG);
            }
            ChuniMessage::JvsFullStateRead => {
                data.push(Self::JVS_FULL_STATE_READ);
            }
            ChuniMessage::JvsFullStateReadResponse {
                opbtn,
                beams,
                pressure,
                coin_counter,
            } => {
                data.push(Self::JVS_FULL_STATE_READ_RESPONSE);
                data.push(*opbtn);
                data.push(*beams);
                data.extend_from_slice(pressure);
                data.extend_from_slice(&coin_counter.to_le_bytes());
            }
        }

        data
    }

    /// Deserialize message from bytes
    pub fn deserialize(data: &[u8]) -> io::Result<Self> {
        if data.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Empty data"));
        }

        let mut cursor = Cursor::new(data);
        let mut message_type = [0u8; 1];
        cursor.read_exact(&mut message_type)?;

        match message_type[0] {
            Self::JVS_POLL => Ok(ChuniMessage::JvsPoll),
            Self::JVS_POLL_RESPONSE => {
                let mut opbtn = [0u8; 1];
                let mut beams = [0u8; 1];
                cursor.read_exact(&mut opbtn)?;
                cursor.read_exact(&mut beams)?;
                Ok(ChuniMessage::JvsPollResponse {
                    opbtn: opbtn[0],
                    beams: beams[0],
                })
            }
            Self::COIN_COUNTER_READ => Ok(ChuniMessage::CoinCounterRead),
            Self::COIN_COUNTER_READ_RESPONSE => {
                let mut count_bytes = [0u8; 2];
                cursor.read_exact(&mut count_bytes)?;
                let count = u16::from_le_bytes(count_bytes);
                Ok(ChuniMessage::CoinCounterReadResponse { count })
            }
            Self::SLIDER_INPUT => {
                let mut pressure = [0u8; 32];
                cursor.read_exact(&mut pressure)?;
                Ok(ChuniMessage::SliderInput { pressure })
            }
            Self::SLIDER_STATE_READ => Ok(ChuniMessage::SliderStateRead),
            Self::SLIDER_STATE_READ_RESPONSE => {
                let mut pressure = [0u8; 32];
                cursor.read_exact(&mut pressure)?;
                Ok(ChuniMessage::SliderStateReadResponse { pressure })
            }
            Self::SLIDER_LED_UPDATE => {
                let mut len_bytes = [0u8; 1];
                cursor.read_exact(&mut len_bytes)?;
                let len = len_bytes[0] as usize;

                let mut rgb_data = vec![0u8; len];
                cursor.read_exact(&mut rgb_data)?;
                Ok(ChuniMessage::SliderLedUpdate { rgb_data })
            }
            Self::LED_UPDATE => {
                let mut board = [0u8; 1];
                cursor.read_exact(&mut board)?;

                let mut len_bytes = [0u8; 1];
                cursor.read_exact(&mut len_bytes)?;
                let len = len_bytes[0] as usize;

                let mut rgb_data = vec![0u8; len];
                cursor.read_exact(&mut rgb_data)?;
                Ok(ChuniMessage::LedUpdate {
                    board: board[0],
                    rgb_data,
                })
            }
            Self::PING => Ok(ChuniMessage::Ping),
            Self::PONG => Ok(ChuniMessage::Pong),
            Self::JVS_FULL_STATE_READ => Ok(ChuniMessage::JvsFullStateRead),
            Self::JVS_FULL_STATE_READ_RESPONSE => {
                let mut opbtn = [0u8; 1];
                let mut beams = [0u8; 1];
                let mut pressure = [0u8; 32];
                let mut coin_bytes = [0u8; 2];
                cursor.read_exact(&mut opbtn)?;
                cursor.read_exact(&mut beams)?;
                cursor.read_exact(&mut pressure)?;
                cursor.read_exact(&mut coin_bytes)?;
                let coin_counter = u16::from_le_bytes(coin_bytes);
                Ok(ChuniMessage::JvsFullStateReadResponse {
                    opbtn: opbtn[0],
                    beams: beams[0],
                    pressure,
                    coin_counter,
                })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unknown message type: {}", message_type[0]),
            )),
        }
    }
}
