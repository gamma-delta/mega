mod audio;
mod mega_state;
use mega_state::MegaState;

fn main() {
    println!("Initializing Mega...");
    let mut mega = MegaState::new();
    println!("Starting Mega...");
    let result = mega.start();
    match result {
        Ok(_) => println!("Mega succesfully exited!"),
        Err(err) => println!("Mega exited with an error! {}", err),
    }
}

// DeepSpeech requires this sample rate.
const DEEPSPEECH_SAMPLE_RATE: u32 = 16_000;
