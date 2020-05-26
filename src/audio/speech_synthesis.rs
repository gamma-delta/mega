//! Exposes a platform-agnostic API for speech synthesis.
//! Wow that was a bunch of fancy developer words.

#[cfg(target_os = "windows")]
pub use windows_version::SpeechSynthesizer;
#[cfg(target_os = "windows")]
mod windows_version {
    use winapi::ctypes;
    use winapi::shared::{guiddef::CLSID, winerror};
    use winapi::um::{
        combaseapi::{self, CLSIDFromProgID, CoCreateInstance, CoInitializeEx},
        objbase::COINIT_MULTITHREADED,
        sapi::{IEnumSpObjectTokens, ISpObjectTokenCategory, ISpVoice},
        winnt::HRESULT,
    };
    use winapi::{Class, Interface};

    use std::{mem, ptr::null_mut, thread};

    pub struct SpeechSynthesizer<'a> {
        synther: &'a mut ISpVoice,
    }

    impl<'a> SpeechSynthesizer<'a> {
        pub fn init() -> Result<Self, String> {
            println!("Initializing Windows speech synthesizer");
            // Intialize COM, which operates on the current thread FOREVER BWAHAHa
            unsafe { CoInitializeEx(null_mut(), COINIT_MULTITHREADED) }.check(line!())?;
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
                CLSIDFromProgID(ptr, &mut clsid).check(line!())?;

                CoCreateInstance(
                    &clsid,
                    null_mut(),
                    combaseapi::CLSCTX_ALL,
                    &ISpVoice::uuidof(),
                    &mut win_synther as *mut *mut ISpVoice as *mut *mut ctypes::c_void,
                )
            }
            .check(line!())?;

            // Get a list of all voices
            /*
            let mut spotc: *mut ISpObjectTokenCategory = null_mut();
            unsafe {
                let mut clsid: CLSID = mem::zeroed();
                let clsid_string = "SAPI.SpObjectTokenCategory".to_string().widen();
                CLSIDFromProgID(clsid_string.as_ptr(), &mut clsid).check(line!())?;

                CoCreateInstance(
                    &clsid,
                    null_mut(),
                    combaseapi::CLSCTX_ALL,
                    &ISpObjectTokenCategory::uuidof(),
                    &mut spotc as *mut *mut ISpObjectTokenCategory as *mut *mut ctypes::c_void,
                )
            }
            .check(line!())?;
            let mut voices: *mut IEnumSpObjectTokens = unsafe { mem::zeroed() };
            unsafe {
                spotc.EnumTokens(
                    SPCAT_VOICES,
                    null_mut(),
                    null_mut(),
                    &mut voices as *mut *mut IEnumSpObjectTokens as *mut *mut ctypes::c_void,
                )
            }
            */

            // Now abstract it away for the end user.
            let synthesizer = SpeechSynthesizer {
                synther: unsafe { &mut *win_synther },
            };

            Ok(synthesizer)
        }

        pub fn speak(&mut self, msg: String) -> Result<(), String> {
            let wide_msg = msg.widen();
            let ptr_to_wide = wide_msg.as_ptr(); // i think you have to pop out the pointer like this to ensure it isn't dropped
            unsafe { self.synther.Speak(ptr_to_wide, 19, null_mut()) }.check(line!())?;
            Ok(())
        }
    }

    trait TraitForHresultChecking {
        fn check(self, line: u32) -> Result<(), String>;
    }

    impl TraitForHresultChecking for HRESULT {
        fn check(self, line: u32) -> Result<(), String> {
            if winerror::SUCCEEDED(self) {
                Ok(())
            } else {
                Err(format!("WinAPI error code `0x{:x}` on line {}", self, line))
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
