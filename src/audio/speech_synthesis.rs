//! Exposes a platform-agnostic API for speech synthesis.
//! Wow that was a bunch of fancy developer words.

#[cfg(target_os = "windows")]
pub use windows_version::init;
#[cfg(target_os = "windows")]
mod windows_version {
    use winapi::ctypes;
    use winapi::shared::{guiddef::CLSID, winerror};
    use winapi::um::{
        combaseapi::{self, CLSIDFromProgID, CoCreateInstance, CoInitializeEx},
        objbase::COINIT_MULTITHREADED,
        sapi::ISpVoice,
        winnt::HRESULT,
    };
    use winapi::Interface;

    use crossbeam::channel;

    use std::{mem, ptr::null_mut, thread};

    pub fn init() -> Result<(channel::Sender<String>, thread::JoinHandle<()>), String> {
        println!("Initializing Windows speech synthesizer");

        // Abstract it away for the end user.
        let (sender, receiver) = channel::unbounded::<String>();
        // Spin up a thread to handle the receiving
        let handle = thread::spawn(move || {
            // Intialize COM, which operates on the current thread FOREVER BWAHAHa
            unsafe { CoInitializeEx(null_mut(), COINIT_MULTITHREADED) }
                .check(line!())
                .unwrap();
            // Gonna be honest, no idea what any of this means
            let mut win_synther: *mut ISpVoice = null_mut();

            // Initiate the ISpVoice itself!
            unsafe {
                // Init the CLSID for SpVoice
                let mut clsid: CLSID = mem::zeroed();
                // this magic string taken from:
                // https://github.com/Eh2406/rust-reader/blob/9e0d1496d7ddccb80005b37261eeea5f08cf90a0/src/sapi.rs#L69
                let clsid_string = "SAPI.SpVoice".to_string().widen();
                let ptr = clsid_string.as_ptr();
                CLSIDFromProgID(ptr, &mut clsid).check(line!()).unwrap();

                CoCreateInstance(
                    &clsid,
                    null_mut(),
                    combaseapi::CLSCTX_ALL,
                    &ISpVoice::uuidof(),
                    &mut win_synther as *mut *mut ISpVoice as *mut *mut ctypes::c_void,
                )
            }
            .check(line!())
            .unwrap();
            let synther = unsafe { &mut *win_synther };
            loop {
                let msg = receiver.recv().unwrap();
                let wide_msg = msg.widen();
                let ptr_to_wide = wide_msg.as_ptr(); // i think you have to pop out the pointer like this to ensure it isn't dropped
                unsafe { synther.Speak(ptr_to_wide, 19, null_mut()) }
                    .check(line!())
                    .unwrap();
            }
        });

        Ok((sender, handle))
    }

    trait TraitForHresultChecking {
        fn check(self, line: u32) -> Result<(), String>;
    }

    impl TraitForHresultChecking for HRESULT {
        fn check(self, line: u32) -> Result<(), String> {
            if winerror::SUCCEEDED(self) {
                Ok(())
            } else {
                Err(format!("WinAPI error code `{:#x}` on line {}", self, line))
            }
        }
    }

    trait WidenableString {
        fn widen(self) -> Vec<u16>;
    }
    impl WidenableString for String {
        /// Use `.as_ptr()` to get the winapi-friendly version
        fn widen(self) -> Vec<u16> {
            let mut wide: Vec<u16> = self.encode_utf16().collect(); // o lawd it wide
            wide.push(0); // null-terminate it
            wide
        }
    }
}
