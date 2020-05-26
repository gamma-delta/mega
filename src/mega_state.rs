use crate::audio;
use crate::DEEPSPEECH_SAMPLE_RATE;

use audrey::read::Reader;
#[allow(unused_imports)]
use audrey::sample::{
    interpolate::{Converter, Linear, Sinc},
    ring_buffer::Fixed as RingBuffer,
    signal::{self, Signal},
    Sample,
};
use deepspeech::{Metadata, Model};

use std::path::Path;
use std::sync::mpsc;
use std::time;
use std::{collections::VecDeque, thread};

/// How loud you have to be for Mega to count you as speaking
const ACTIVATION_THRESHOLD: f64 = 0.01;
/// The number of transcripts Mega will output per utterance.
/// Making this larger will make Mega produce more guesses.
/// Lower down on the list of guesses, it gets more wild.
const TRANSCRIPT_COUNT: u16 = 10;
/// The amount of time you must be loud or quiet for for Mega to start speech processing
const THRESHOLD_TIME_SECONDS: f64 = 1.0;
/// The `lambda` parameter in `tvid::condat`
const DENOISE_RADIUS: f32 = 0.05;

/// The amount of time it buffers while listening for "Mega"
const ACTIVATION_BUFFER_SIZE_SECONDS: f64 = 2.0;
/// The amount of time Mega saves while listening to a command.
const COMMAND_BUFFER_SIZE_SECONDS: f64 = 15.0;

/// MegaState handles the state of the Mega instance.
#[allow(dead_code)]
pub struct MegaState<'a> {
    speaker_sender: mpsc::Sender<Vec<f32>>,
    speaker_thread_handle: thread::JoinHandle<()>,
    speaker_sample_rate: u32,
    mic_receiver: mpsc::Receiver<Vec<f32>>,
    mic_thread_handle: thread::JoinHandle<()>,
    mic_sample_rate: u32,

    /// Speech to text
    speech_model: Model,

    /// Text to speech
    speech_synther: audio::SpeechSynthesizer<'a>,

    /// State machine
    state: State,
}

impl<'a> MegaState<'a> {
    /// Return a new MegaState ready for running
    pub fn new() -> Self {
        // Spin up the audio
        let (
            (speaker_sender, speaker_thread_handle, speaker_sample_rate),
            (mic_receiver, mic_thread_handle, mic_sample_rate),
        ) = audio::get_audio_channels();

        // Initialize the model
        let mut speech_model =
            Model::load_from_files(Path::new("resources/deepspeech-0.7.0-models.pbmm"))
                .expect("Could not open DeepSpeech model file!");
        // Enable scoring (which will make it better, I hope?)
        // speech_model.enable_external_scorer(Path::new("resources/deepspeech-0.7.1-models.scorer"));

        // TEST
        #[allow(non_upper_case_globals)]
        const do_test: bool = false;
        if do_test {
            let test_audio = Reader::new(
                std::fs::File::open("resources/test_audio/2830-3980-0043.wav").unwrap(),
            )
            .unwrap()
            .samples()
            .map(|s| s.unwrap())
            .collect::<Vec<_>>();
            let test_text = speech_model.speech_to_text(&test_audio).unwrap();
            println!("DeepSpeech test: {}", test_text);
        }

        // Init speech synthesizer
        let speech_synther = audio::SpeechSynthesizer::init().unwrap();

        // Init state
        let state = State::new_idle(speaker_sample_rate as f64);

        println!("Mega initialized!");

        Self {
            speaker_sender,
            speaker_thread_handle,
            speaker_sample_rate,
            mic_receiver,
            mic_thread_handle,
            mic_sample_rate,
            speech_model,
            speech_synther,
            state,
        }
    }

    /// Starts Mega!
    /// This will block forever until something horrible happens.
    pub fn start(&mut self) -> Result<(), String> {
        loop {
            match self.state {
                State::Idle {
                    ref mut audio_buffer,
                    buf_size,
                    loudness_check_size,
                    ref mut crossed_loudness,
                } => {
                    // Get the next audio bits from the microphone
                    let new_audio = self.mic_receiver.try_iter();

                    // Buffer in the new audio
                    buffer_audio(audio_buffer, new_audio, buf_size);

                    // See if it's LOUD ENOUGH to warrant trying to scan for words
                    let loudness: f64 = audio_buffer
                        .iter()
                        .rev()
                        .take(loudness_check_size)
                        .fold(0.0, |acc, &sample| acc + sample.abs() as f64);
                    let avg_loudness = loudness / loudness_check_size as f64;
                    // println!("Loudness: {}", avg_loudness);

                    if !*crossed_loudness && avg_loudness >= ACTIVATION_THRESHOLD {
                        // OK, it's worth listening!
                        *crossed_loudness = true;
                    } else if *crossed_loudness && avg_loudness < ACTIVATION_THRESHOLD {
                        // We're done speaking; let's-a go!
                        *crossed_loudness = false;
                        print!("Processing... ");
                        flush();

                        let (speech, dur) = MegaState::text_to_speech(
                            &mut self.speech_model,
                            audio_buffer.iter().cloned(),
                            self.mic_sample_rate,
                        )?;
                        print!(
                            "in {:4} seconds ({}% of RT): ",
                            dur.as_secs_f64(),
                            100.0 * dur.as_secs_f64() / ACTIVATION_BUFFER_SIZE_SECONDS
                        );
                        let found_mega = speech.transcripts().iter().any(|tc| {
                            // Confusingly, tc.tokens() yields the separate letters.
                            // Perhaps in other natlangs they mean something different?
                            print!("{}, ", tc);
                            tc.to_string() == "mega"
                        });
                        if found_mega {
                            print!("Found \"mega\"!");
                            self.speak("ready".into())?;
                            self.state = State::new_heard_trigger(
                                self.mic_sample_rate as f64,
                                &mut self.speech_model,
                            )?;
                        }
                        println!("");
                    }
                }
                State::HeardTrigger {
                    ref mut deep_stream,
                    ref mut tail_buffer,
                    ref mut crossed_loudness,
                    loudness_check_size,
                } => {
                    // Get the next audio bits from the microphone
                    // Collect it now so there's no borrow problems
                    let new_audio = self.mic_receiver.try_iter().collect::<Vec<_>>();
                    // Flatten it
                    let flattened = new_audio.clone().into_iter().flatten().collect::<Vec<_>>();

                    // Convert the new audio for deepspeech
                    let audio_wip = tv1d::tautstring(&flattened, DENOISE_RADIUS);
                    // Convert audio to a Signal
                    let sig = signal::from_iter(audio_wip.iter().map(|&s| [s.to_sample()]));
                    // convert to i16, 16000hz audio.
                    let interpolator = Linear::new([0i16], [0]);
                    let converter = Converter::from_hz_to_hz(
                        sig,
                        interpolator,
                        self.mic_sample_rate as f64,
                        DEEPSPEECH_SAMPLE_RATE as f64,
                    );
                    let converted = converter
                        .until_exhausted()
                        .map(|s| s[0])
                        .collect::<Vec<_>>();
                    // pipe it in
                    deep_stream.feed_audio(&converted);

                    // See if it's LOUD ENOUGH to warrant trying to scan for words
                    // Buffer it in for averages
                    buffer_audio(tail_buffer, new_audio.into_iter(), loudness_check_size);
                    let loudness: f64 = tail_buffer
                        .iter()
                        .rev()
                        .take(loudness_check_size)
                        .fold(0.0, |acc, &sample| acc + sample.abs() as f64);
                    let avg_loudness = loudness / loudness_check_size as f64;

                    // Possibly calculate this dynamically later?
                    if !*crossed_loudness && avg_loudness >= ACTIVATION_THRESHOLD {
                        // OK, it's worth listening!
                        *crossed_loudness = true;
                    } else if *crossed_loudness && avg_loudness < ACTIVATION_THRESHOLD {
                        // We're done speaking; let's-a go!
                        *crossed_loudness = false;
                        println!("Processing command... ");

                        // The stream finish_with_metadata takes a u32, while the normal one takes a u16
                        // literally unplayable
                        let speech = deep_stream
                            .finish_with_metadata(TRANSCRIPT_COUNT as u32)
                            .map_err(|_| "DeepSpeech could not finish processing streamed audio")?;

                        // Marshall the heard audio.
                        for trans /*rights*/ in speech.transcripts() {
                            let trans_str = trans.to_string(); // curse you dropped values
                            let split = trans_str.split_whitespace().collect::<Vec<_>>();
                            println!("* {:?}", split);
                        }
                    }
                }
            };
        }
    }

    /// Does text-to-speech
    fn text_to_speech<I>(
        speech_model: &mut Model,
        audio_data: I,
        mic_sample_rate: u32,
    ) -> Result<(Metadata, time::Duration), String>
    where
        I: IntoIterator<Item = f32>,
    {
        let audio_wip =
            tv1d::tautstring(&audio_data.into_iter().collect::<Vec<_>>(), DENOISE_RADIUS);

        // Convert audio to a Signal
        let sig = signal::from_iter(audio_wip.iter().map(|&s| [s.to_sample()]));

        // convert to i16, 16000hz audio.
        let interpolator = Linear::new([0i16], [0]);
        let converter = Converter::from_hz_to_hz(
            sig,
            interpolator,
            mic_sample_rate as f64,
            DEEPSPEECH_SAMPLE_RATE as f64,
        );

        let converted = converter
            .until_exhausted()
            .map(|s| s[0])
            .collect::<Vec<_>>();

        let now = std::time::Instant::now();
        let speech = speech_model
            .speech_to_text_with_metadata(&converted, TRANSCRIPT_COUNT)
            .map_err(|_| "deepspeech had an unknown error while parsing text")?;
        let elapsed = now.elapsed();
        Ok((speech, elapsed))
    }

    /// Makes Mega say something
    fn speak(&mut self, msg: String) -> Result<(), String> {
        self.speech_synther.speak(msg)
    }
}

/// Used for MegaState's state machine
enum State {
    /// Waiting for "Mega"
    Idle {
        /// Buffers the audio heard
        audio_buffer: VecDeque<f32>,
        /// How long (in samples) the buffered audio should be
        buf_size: usize,
        /// How much of the newest of the audio we should check for loudness
        loudness_check_size: usize,
        /// Keeps track of whether we've gone over the loudness threshold.
        /// false if we haven't passed it; true if we have
        crossed_loudness: bool,
    },
    /// Heard "Mega", now waiting for commands
    HeardTrigger {
        /// The Stream for DeepSpeech
        deep_stream: deepspeech::Stream,
        /// The tail end of the stream sent for averaging
        tail_buffer: VecDeque<f32>,
        /// How much of the newest of the audio we should check for loudness
        loudness_check_size: usize,
        /// Whether we crossed the loudness threshold
        crossed_loudness: bool,
    },
}

impl State {
    fn new_idle(sample_rate: f64) -> Self {
        let buf_size = (sample_rate * ACTIVATION_BUFFER_SIZE_SECONDS) as usize;
        let loudness_check_size = (sample_rate * THRESHOLD_TIME_SECONDS) as usize;
        let audio_buffer = (0..buf_size).map(|_| 0.0).collect();
        State::Idle {
            buf_size,
            loudness_check_size,
            audio_buffer,
            crossed_loudness: false,
        }
    }
    fn new_heard_trigger(sample_rate: f64, speech_model: &mut Model) -> Result<Self, String> {
        let loudness_check_size = (sample_rate * THRESHOLD_TIME_SECONDS) as usize;
        let tail_buffer = (0..loudness_check_size).map(|_| 0.0).collect();
        let stream = speech_model
            .create_stream()
            .map_err(|_| "could not start DeepSpeech streaming")?;
        Ok(State::HeardTrigger {
            tail_buffer,
            deep_stream: stream,
            loudness_check_size,
            crossed_loudness: false,
        })
    }
}

// Helper functions

/// Add new audio data to the VecDeque, and pop data from the front until it's the given size.
fn buffer_audio<T, I>(buffer: &mut VecDeque<T>, new_data: I, buf_size: usize)
where
    I: Iterator<Item = Vec<T>>,
{
    // Append the newest audio to the back, so it's the last read.
    for snippet in new_data {
        buffer.extend(snippet);
    }
    // Remove the oldest bits at the front
    while buffer.len() > buf_size {
        buffer.pop_front();
    }
}

fn flush() {
    use std::io::Write;
    std::io::stdout().flush().unwrap();
}
