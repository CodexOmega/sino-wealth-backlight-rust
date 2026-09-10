use glob::{glob, GlobError, PatternError};
use libc::{c_int, c_void};
use nix::fcntl::{open, OFlag};
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::stat::Mode;
use nix::unistd::{close, read};
use std::fs;
use std::io::{self, Write};
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

const TARGET_NAME: &str = "SINO WEALTH USB KEYBOARD";
const BY_ID: &str = "/dev/input/by-id/usb-SINO_WEALTH_USB_KEYBOARD-event-kbd";

const EV_KEY: u16 = 0x01;
const KEY_SCROLLLOCK: u16 = 70;
const KEY_PRESS: i32 = 1;

const INPUT_EVENT_SIZE: usize = 24;
const POLL_INTERVAL: Duration = Duration::from_millis(1);
const REASSERT_LOG_INTERVAL: Duration = Duration::from_secs(30);

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct InputEvent {
    time_sec: i64,
    time_usec: i64,
    type_: u16,
    code: u16,
    value: i32,
}

#[derive(Error, Debug)]
enum BacklightError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Nix error: {0}")]
    Nix(#[from] nix::Error),
    #[error("Device not found")]
    DeviceNotFound,
    #[error("LED path not found")]
    LedPathNotFound,
    #[error("Invalid brightness value")]
    InvalidBrightness,
    #[error("Glob pattern error: {0}")]
    GlobPattern(#[from] PatternError),
    #[error("Glob error: {0}")]
    Glob(#[from] GlobError),
}

fn log(msg: &str) {
    println!("{}", msg);
    io::stdout().flush().ok();
}

fn find_led_brightness() -> Result<PathBuf, BacklightError> {
    for entry in glob("/sys/class/leds/*::scrolllock")? {
        let led_path = entry?;
        let device_path = led_path.join("device");
        let real_device = match fs::read_link(&device_path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let device_dir = device_path.parent().unwrap();
        let name_path = if real_device.is_absolute() {
            real_device.join("name")
        } else {
            device_dir.join(&real_device).join("name")
        };
        let name_path = match name_path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if let Ok(name) = fs::read_to_string(&name_path) {
            if name.trim() == TARGET_NAME {
                return Ok(led_path.join("brightness"));
            }
        }
    }
    Err(BacklightError::LedPathNotFound)
}

fn find_evdev() -> Result<PathBuf, BacklightError> {
    let mut candidates = Vec::new();

    if Path::new(BY_ID).exists() {
        candidates.push(PathBuf::from(BY_ID));
    }

    if candidates.is_empty() {
        for entry in glob("/dev/input/event*")? {
            let ev_path = entry?;
            if let Ok(fd) = open(&ev_path, OFlag::O_RDONLY | OFlag::O_NONBLOCK, Mode::empty()) {
                let name = get_evdev_name(fd)?;
                close(fd).ok();
                if name == TARGET_NAME {
                    candidates.push(ev_path);
                }
            }
        }
    }

    candidates
        .into_iter()
        .next()
        .ok_or(BacklightError::DeviceNotFound)
}

fn get_evdev_name(fd: c_int) -> Result<String, BacklightError> {
    const EVIOCGNAME_LEN: c_int = 256;
    let mut name_buf = vec![0u8; EVIOCGNAME_LEN as usize];

    unsafe {
        let request = (2u32 << 30) | (0x45u32 << 8) | 0x06 | (256u32 << 16);
        let ret = libc::ioctl(fd, request as _, name_buf.as_mut_ptr() as *mut c_void);
        if ret < 0 {
            return Err(BacklightError::Io(io::Error::last_os_error()));
        }
        let len = ret as usize;
        Ok(String::from_utf8_lossy(&name_buf[..len]).to_string())
    }
}

fn read_brightness(path: &Path) -> Result<i32, BacklightError> {
    let content = fs::read_to_string(path)?;
    content
        .trim()
        .parse()
        .map_err(|_| BacklightError::InvalidBrightness)
}

fn write_brightness(path: &Path, value: i32) -> Result<(), BacklightError> {
    let content = if value != 0 { "1" } else { "0" };
    fs::write(path, content)?;
    Ok(())
}

fn set_trigger_none() {
    if let Ok(entries) = glob("/sys/class/leds/*::scrolllock") {
        for entry in entries.flatten() {
            let trigger_path = entry.join("trigger");
            if let Ok(content) = fs::read_to_string(&trigger_path) {
                if !content.contains("[none]") {
                    fs::write(&trigger_path, "none").ok();
                }
            }
        }
    }
}

fn main() -> Result<(), BacklightError> {
    log(&format!(
        "{} starting: backlight initial state ON",
        std::env::args().next().unwrap_or_else(|| "sino-wealth-backlight".to_string())
    ));

    set_trigger_none();
    let mut desired = 1;

    let mut led_path: Option<PathBuf> = None;
    let mut evfd: Option<c_int> = None;
    let mut last_poll = Instant::now();
    let mut last_reassert_log = Instant::now();

    loop {
        if led_path.is_none() {
            match find_led_brightness() {
                Ok(path) => led_path = Some(path),
                Err(_) => {
                    log("waiting for scrolllock LED sysfs entry...");
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            }
        }

        if evfd.is_none() {
            match find_evdev() {
                Ok(path) => {
                    let fd = open(&path, OFlag::O_RDONLY | OFlag::O_NONBLOCK, Mode::empty())?;
                    evfd = Some(fd);
                    log(&format!("watching {}", path.display()));
                }
                Err(_) => {
                    log("waiting for keyboard evdev device...");
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            }
        }

        let now = Instant::now();
        if now.duration_since(last_poll) >= POLL_INTERVAL {
            last_poll = now;
            if let Some(ref path) = led_path {
                match read_brightness(path) {
                    Ok(current) if current != desired => {
                        if now.duration_since(last_reassert_log) >= REASSERT_LOG_INTERVAL {
                            last_reassert_log = now;
                            log(&format!(
                                "re-asserting backlight -> {}",
                                if desired != 0 { "ON" } else { "OFF" }
                            ));
                        }
                        if write_brightness(path, desired).is_err() {
                            led_path = None;
                        }
                    }
                    Err(_) => {
                        led_path = None;
                    }
                    _ => {}
                }
            }
        }

        let fd = match evfd {
            Some(fd) => fd,
            None => continue,
        };

        let mut poll_fds = [PollFd::new(unsafe { BorrowedFd::borrow_raw(fd) }, PollFlags::POLLIN)];
        match poll(&mut poll_fds, POLL_INTERVAL.as_millis() as u16) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(_) => {
                evfd = None;
                continue;
            }
        }

        let mut buffer = [0u8; INPUT_EVENT_SIZE * 32];
        let n = match read(fd, &mut buffer) {
            Ok(n) => n,
            Err(_) => {
                evfd = None;
                continue;
            }
        };

        let mut offset = 0;
        while offset + INPUT_EVENT_SIZE <= n {
            let event = unsafe { *(buffer[offset..].as_ptr() as *const InputEvent) };
            if event.type_ == EV_KEY && event.code == KEY_SCROLLLOCK && event.value == KEY_PRESS {
                desired ^= 1;
                log(&format!(
                    "scroll lock pressed: backlight -> {}",
                    if desired != 0 { "ON" } else { "OFF" }
                ));
                if let Some(ref path) = led_path {
                    write_brightness(path, desired).ok();
                }
            }
            offset += INPUT_EVENT_SIZE;
        }
    }
}