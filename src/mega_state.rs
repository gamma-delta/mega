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
use rlua::{Lua, Error as LuaError, Table};
use crossbeam::channel;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time;
use std::{
    collections::{HashMap, VecDeque},
    thread,
};
use std::fs;
use audio::speech_synthesis;

/// How loud you have to be for Mega to count you as speaking
const ACTIVATION_THRESHOLD: f64 = 0.01;
/// The number of transcripts Mega will output per utterance.
/// Making this larger will make Mega produce more guesses.
/// Lower down on the list of guesses, it gets more wild.
const TRANSCRIPT_COUNT: u16 = 300;
/// The amount of time you must be loud or quiet for for Mega to start speech processing
const THRESHOLD_TIME_SECONDS: f64 = 1.0;
/// The `lambda` parameter in `tvid::condat`
const DENOISE_RADIUS: f32 = 0.05;

/// The amount of time it buffers while listening for "Mega"
const ACTIVATION_BUFFER_SIZE_SECONDS: f64 = 2.0;
/// The amount of time it buffers while listening for a command
const COMMAND_BUFFER_SIZE_SECONDS: f64 = 15.0;

/// MegaState handles the state of the Mega instance.
#[allow(dead_code)]
pub struct MegaState {
    speaker_sender: mpsc::Sender<Vec<f32>>,
    speaker_thread_handle: thread::JoinHandle<()>,
    speaker_sample_rate: u32,
    mic_receiver: mpsc::Receiver<Vec<f32>>,
    mic_thread_handle: thread::JoinHandle<()>,
    mic_sample_rate: u32,

    /// Speech to text
    speech_model: Model,

    /// Text to speech
    synther_sender: channel::Sender<String>,
    synther_thread_handle: thread::JoinHandle<()>,

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

        // Init speech synthesizer
        let (synther_sender, synther_thread_handle) = speech_synthesis::init().unwrap();

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
            synther_sender,
            synther_thread_handle,
            state,
        }
    }

    /// Starts Mega!
    /// This will block forever until something horrible happens.
    pub fn start(&mut self) -> Result<(), String> {
        'main: loop {
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
                            "in {:.2} seconds ({:.0}% of RT): ",
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
                            self.speak("ready")?;
                            self.state = State::new_heard_trigger(self.mic_sample_rate as f64);
                        }
                        println!("");
                    }
                }
                State::HeardTrigger {
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
                        print!("Processing command... ");
                        flush();

                        // Send only the parts starting when it goes above the threshold to DeepSpeech
                        let mut running_loudness = 0.0;
                        let start_index =
                            audio_buffer
                                .iter()
                                .enumerate()
                                .find_map(|(idx, &sample)| {
                                    // Compute the running average
                                    // Add the newest divided by the length,
                                    // subtract the oldest divided by the length.
                                    running_loudness += (sample.abs()
                                        - match idx.checked_sub(loudness_check_size) {
                                            Some(idx) => audio_buffer[idx], 
                                            None => 0.0
                                        }.abs())
                                        / loudness_check_size as f32;
                                    // Check if we're loud enough
                                    if running_loudness >= ACTIVATION_THRESHOLD as f32 {
                                        Some(idx.saturating_sub(loudness_check_size / 2))
                                    } else {
                                        None
                                    }
                                }).ok_or_else(|| "Somehow tried to process a command both loud enough and not loud enough")?;

                        print!("Starting at index {}... ", start_index);
                        flush();
                        // self.speaker_sender.send(audio_buffer.iter().skip(start_index).cloned().collect()).map_err(|err| err.to_string())?;

                        let (speech, dur) = MegaState::text_to_speech(
                            &mut self.speech_model,
                            audio_buffer.iter().skip(start_index).cloned(),
                            self.mic_sample_rate,
                        )?;
                        println!(
                            "in {:.2} seconds ({:.0}% of RT):",
                            dur.as_secs_f64(),
                            100.0 * dur.as_secs_f64()
                                / ((audio_buffer.len() - start_index)
                                    / self.mic_sample_rate as usize)
                                    as f64
                        );

                        // First process into a HashMap indexed by (depth, certainty)
                        let mut tree_map: HashMap<(usize, usize), String> = HashMap::new();
                        let mut max_certainty = 0;
                        let mut max_depth = 0;
                        for (certainty_idx, tc) in speech.transcripts().iter().enumerate() {
                            max_certainty = max_certainty.max(certainty_idx);
                            // This goes depth-first, but we want breadth-first.
                            let sentence = tc.to_string();
                            let split = sentence.split_whitespace();
                            for (depth_idx, word) in split.enumerate() {
                                max_depth = max_depth.max(depth_idx);
                                tree_map.insert((depth_idx, certainty_idx), word.to_string());
                            }
                        }
                        // println!("{:?}", &tree_map);

                        // Flatten into a Vec<Vec<String>>
                        // First level is all the possibilites for this depth in the tree.
                        let mut tree: Vec<Vec<Option<String>>> =
                            (0..=max_depth).map(|_| vec![None; max_certainty]).collect();
                        for certainty_idx in 0..max_certainty {
                            for depth_idx in 0..=max_depth {
                                let word = tree_map.remove(&(depth_idx, certainty_idx));
                                tree[depth_idx][certainty_idx] = word;
                            }
                        }
                        self.speak("searching for command")?;

                        // To the bat-command!
                        self.state = State::new_searching_for_command(tree);
                    }
                }
                State::SearchingForCommand { ref command } => {
                    // The path that we know is all OK.
                    let mut path = PathBuf::from("commands");
                    'level: for (idx, possibilities) in command.iter().enumerate() {
                        for poss in possibilities {
                            let try_path = path.join(poss.as_ref().unwrap_or(&"".to_string()));
                            println!("Trying path {:?} ", try_path);
                            if try_path.exists() {
                                if try_path.is_dir() {
                                    // Nice, a folder! Let's keep going
                                    path = try_path;
                                    continue 'level;
                                } else {
                                    // what are you doing?
                                    let msg = format!("Invalid file found in commands: {:?}", try_path.file_name());
                                    println!("{}", msg.clone());
                                    self.speak(msg)?;
                                    self.state = State::new_idle(self.mic_sample_rate as f64);
                                    continue 'main;
                                }
                            } else {
                                // But perhaps it's the path to a lua file?
                                let luaed_path = path.join(format!("{}.lua", poss.as_ref().unwrap_or(&"".to_string())));
                                if let Ok(md) = fs::metadata(luaed_path.clone()) {
                                    // Hey, there's a file here! Or an oddly named folder.
                                    if md.is_file() {
                                        // Awesome we found the command~!
                                        // Fill the arguments
                                        let args = command
                                            .iter()
                                            .skip(idx + 1)
                                            .map(|possibilities| 
                                                possibilities
                                                .iter()
                                                .filter_map(|it| it.clone())
                                                .collect::<Vec<_>>()
                                            )
                                            .collect::<Vec<Vec<String>>>();

                                        self.speak("Executing command")?;
                                        println!(
                                            "Found command! {:?} with {:?}",
                                            luaed_path.clone(),
                                            args.clone()
                                        );
                                        self.state = State::new_execing_command(
                                            self.synther_sender.clone(), 
                                            luaed_path, 
                                            args
                                        )?;

                                        continue 'main;
                                    } else {
                                        // Else we have a *folder* named that.lua. Why would you do that?
                                    }
                                } else {
                                    // Well, that attempt wasn't valid. Back to try another possibility.
                                }
                            }
                        }
                        // We ran out of paths ;(
                        self.speak("Could not find that command.")?;
                        println!("Failed to find the command after: {:?}", path);
                        self.state = State::new_idle(self.mic_sample_rate as f64);
                        continue 'main;
                    }
                    // Not sure how you get here, but i know it means you're out of possible commands
                    self.speak("Could not find that command.")?;
                    println!("Failed to find the command after: {:?}", path);
                    self.state = State::new_idle(self.mic_sample_rate as f64);
                    continue 'main;
                }
                State::ExecingCommand { ref path, ref args, ref lua_state } => {
                    lua_state.context(|ctx| {
                        let globals = ctx.globals();
                        let mega_api = globals.get::<_, Table>("Mega")?;

                        // Add the arguments to `Mega.arguments` and `Mega.raw_arguments`
                        let mega_arguments = ctx.create_table()?;
                        let mega_raw_arguments = ctx.create_table()?;
                        for (arg_idx, possible_args) in args.iter().enumerate() {
                            let mega_possibilities = ctx.create_table()?;
                            for (possible_idx, possibility) in possible_args.iter().enumerate() {
                                if possible_idx == 0 {
                                    // add 1 to everything, because lua counts at 1...
                                    mega_arguments.set(arg_idx + 1, possibility.clone())?;
                                }
                                mega_possibilities.set(possible_idx + 1, possibility.clone())?;
                            }
                            mega_raw_arguments.set(arg_idx + 1, mega_possibilities)?;
                        }
                        mega_api.set("arguments", mega_arguments)?;
                        mega_api.set("raw_arguments", mega_raw_arguments)?;

                        // Load the command file and execute it
                        let file = fs::read(path).unwrap(); // we know the file exists
                        ctx.load(
                            &file
                        ).set_name("Mega api")?
                        .exec()?;

                        Ok(())
                    }).map_err(|err: LuaError| err.to_string())?;

                    self.state = State::new_idle(self.mic_sample_rate as f64);
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
    fn speak<S>(&mut self, msg: S) -> Result<(), String>
    where
        S: Into<String>,
    {
        self.synther_sender.send(msg.into()).map_err(|err| err.to_string())
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
        // Hm these comments look familar
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
    /// Searching the file tree for a command to execute
    SearchingForCommand { command: Vec<Vec<Option<String>>> },
    /// Executing the command
    ExecingCommand { 
        path: PathBuf, 
        args: Vec<Vec<String>>,
        lua_state: Lua,
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
    fn new_heard_trigger(sample_rate: f64) -> Self {
        let buf_size = (sample_rate * COMMAND_BUFFER_SIZE_SECONDS) as usize;
        let loudness_check_size = (sample_rate * THRESHOLD_TIME_SECONDS) as usize;
        let audio_buffer = (0..buf_size).map(|_| 0.0).collect();

        State::HeardTrigger {
            buf_size,
            loudness_check_size,
            audio_buffer,
            crossed_loudness: false,
        }
    }
    fn new_searching_for_command(command: Vec<Vec<Option<String>>>) -> Self {
        State::SearchingForCommand { command }
    }
    fn new_execing_command(speaker: channel::Sender<String>, path: PathBuf, args: Vec<Vec<String>>) -> Result<Self, String> {
        // Initialize Lua
        let lua_state = Lua::new();
        lua_state.context(move |ctx| {
            // Initialize the Mega api!
            let mega_api = ctx.create_table()?;
            // Mega.speak
            let speak = ctx.create_function(move |_, (msg,): (String,)| {
                    let _res = speaker.send(msg);
                    // TODO: ERRORS?
                    Ok(())
            })?;
            mega_api.set("speak", speak)?;

            // Seed the random generator
            ctx.load("math.randomseed(os.time())").set_name("random seeder")?.exec()?;

            // Give Lua access to the Mega api table
            ctx.globals().set("Mega", mega_api)?;
            Ok(())
        }).map_err(|err: LuaError| err.to_string())?;

        Ok(State::ExecingCommand { path, args, lua_state })
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