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
use deepspeech::Model;

use std::path::Path;
use std::sync::mpsc;
use std::{collections::VecDeque, thread};

const BUFFER_SIZE_SECONDS: f64 = 3.0;
const THRESHOLD_TIME_SECONDS: f64 = 1.0;

/// MegaState handles the state of the Mega instance.
#[allow(dead_code)]
pub struct MegaState {
    speaker_sender: mpsc::Sender<Vec<f32>>,
    speaker_thread_handle: thread::JoinHandle<()>,
    speaker_sample_rate: u32,
    mic_receiver: mpsc::Receiver<Vec<f32>>,
    mic_thread_handle: thread::JoinHandle<()>,
    mic_sample_rate: u32,

    speech_model: Model,

    /// State machine
    state: State,
}

impl MegaState {
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
        speech_model.enable_external_scorer(Path::new("resources/deepspeech-0.7.1-models.scorer"));

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

                    // Append the newest audio to the back, so it's the last read.
                    for snippet in new_audio {
                        audio_buffer.extend(snippet);
                    }
                    // Remove the oldest bits at the front
                    while audio_buffer.len() > buf_size {
                        audio_buffer.pop_front();
                    }

                    // See if it's LOUD ENOUGH to warrant trying to scan for words
                    let loudness: f64 = audio_buffer
                        .iter()
                        .rev()
                        .take(loudness_check_size)
                        .fold(0.0, |acc, &sample| acc + sample.abs() as f64);
                    let avg_loudness = loudness / buf_size as f64;
                    // println!("Loudness: {}", avg_loudness);

                    // Possibly calculate this dynamically later?
                    const ACTIVATION_THRESHOLD: f64 = 0.01;
                    if !*crossed_loudness && avg_loudness >= ACTIVATION_THRESHOLD {
                        // OK, it's worth listening!
                        *crossed_loudness = true;
                    } else if *crossed_loudness && avg_loudness < ACTIVATION_THRESHOLD {
                        // We're done speaking; let's-a go!
                        *crossed_loudness = false;
                        println!("Processing...");

                        // Output the buffer
                        self.speaker_sender
                            .send(audio_buffer.iter().cloned().collect())
                            .map_err(|err| err.to_string())?;

                        const DENOISE_RADIUS: f32 = 0.05;
                        let audio_wip = tv1d::tautstring(
                            &audio_buffer.iter().cloned().collect::<Vec<_>>(),
                            DENOISE_RADIUS,
                        );
                        /*
                            let audio_wip = audio_wip.iter().enumerate().map(|(idx, _)| {
                                let surrounding_samples = (0..DENOISE_RADIUS * 2 + 1).map(|raw_id| {
                                    // Out-of-bounds indices just use the first and last values.
                                    match (idx + raw_id).checked_sub(DENOISE_RADIUS) {
                                        Some(it) => *audio_wip
                                            .get(it)
                                            .unwrap_or_else(|| audio_wip.last().unwrap()),
                                        None => audio_wip[0], // We're trying to go into negative indices, so just return the first value
                                    }
                                });
                                let len = surrounding_samples.len();
                                // Use a chonky i128 to avoid overflow errors
                                let sum = surrounding_samples.fold(0i128, |acc, s| acc + s as i128);
                                (sum / len as i128) as i16
                            });
                        */

                        // Convert audio to a Signal
                        let sig = signal::from_iter(audio_wip.iter().map(|&s| [s.to_sample()]));

                        // convert to i16, 16000hz audio.
                        let interpolator = Linear::new([0i16], [0]);
                        /*
                        Sinc::new(RingBuffer::from(vec![
                            [0];
                            // Must have a length of twice the wanted interpolation depth
                            (DEEPSPEECH_SAMPLE_RATE * 2)
                                as usize
                        ]));
                        */
                        let converter = Converter::from_hz_to_hz(
                            sig, // TODO: take down clone
                            interpolator,
                            self.mic_sample_rate as f64,
                            DEEPSPEECH_SAMPLE_RATE as f64,
                        );

                        let converted = converter
                            .until_exhausted()
                            .map(|s| s[0])
                            .collect::<Vec<_>>();

                        // Save it to disc so i can hear it
                        // this is temporary
                        {
                            // Save to disc
                            use audrey::hound::{SampleFormat, WavSpec, WavWriter};
                            let spec = WavSpec {
                                channels: 1,
                                sample_rate: DEEPSPEECH_SAMPLE_RATE,
                                bits_per_sample: 16,
                                sample_format: SampleFormat::Int,
                            };
                            // Keep trying to make the writer to a new file until it succeeds
                            // Safe to unwrap because we're not gonna run out of numbers...
                            let mut writer = (0..)
                                .find_map(|idx| {
                                    let path = format!("outputs/{}.wav", idx);
                                    if std::path::Path::new(&path).exists() {
                                        None // no overwriting please
                                    } else {
                                        Some(WavWriter::create(path, spec).unwrap())
                                    }
                                })
                                .unwrap();
                            for sample in converted.iter() {
                                writer
                                    .write_sample(*sample)
                                    .map_err(|err| err.to_string())?;
                            }
                            writer.finalize().map_err(|err| err.to_string())?;
                        }

                        // listen for "MEGA"
                        let speech = self
                            .speech_model
                            .speech_to_text(&converted)
                            .map_err(|_| "deepspeech had an unknown error while parsing text")?;
                        println!("Mega says `{}`", speech);
                    }
                }
            };
        }
    }
}

/// Used for MegaState's state machine
#[derive(Debug)]
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
    // /// Heard "Mega", now waiting for commands
    // HeardTrigger,
}

impl State {
    fn new_idle(sample_rate: f64) -> Self {
        let buf_size = (sample_rate * BUFFER_SIZE_SECONDS) as usize;
        let loudness_check_size = (sample_rate * THRESHOLD_TIME_SECONDS) as usize;
        let audio_buffer = (0..buf_size).map(|_| 0.0).collect();
        State::Idle {
            buf_size,
            loudness_check_size,
            audio_buffer,
            crossed_loudness: false,
        }
    }
}
