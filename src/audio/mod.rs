//! Handles the audio

use cpal::{
    self,
    traits::{DeviceTrait, EventLoopTrait, HostTrait},
    StreamData, UnknownTypeInputBuffer, UnknownTypeOutputBuffer,
};

use std::collections::VecDeque;
use std::sync::mpsc;
use std::thread;

/// The Reciever will output things that the microphone recieves.
/// The Sender will send audio to the speakers.
/// Also returns the handle to the threads it spins up so the threads aren't dropped.
/// Also also returns the sample rates of the devices.
/// Probably don't call this more than once.
pub fn get_audio_channels() -> (
    (mpsc::Sender<Vec<f32>>, thread::JoinHandle<()>, u32),
    (mpsc::Receiver<Vec<f32>>, thread::JoinHandle<()>, u32),
) {
    // Get audio
    let audio_host = cpal::default_host();

    // === SPEAKER ===

    let audio_out = audio_host
        .default_output_device()
        .expect("no audio output device available");
    print!("audio out: {}; ", audio_out.name().unwrap());
    let speaker_format = audio_out
        .supported_output_formats()
        .unwrap()
        .next()
        .expect("speaker supports no outputs?")
        .with_max_sample_rate();
    let speaker_event_loop = audio_host.event_loop();
    // Start the output stream
    let speaker_stream_id = speaker_event_loop
        .build_output_stream(&audio_out, &speaker_format)
        .expect("The speaker's format wasn't supported?");
    speaker_event_loop
        .play_stream(speaker_stream_id)
        .expect("failed to play speaker stream");

    // Create the channel
    let (speaker_sender, speaker_receiver) = mpsc::channel::<Vec<f32>>();

    // Spin up the thread
    let speaker_handle = thread::spawn(move || {
        let mut master_buffer = VecDeque::new();
        speaker_event_loop.run(move |stream_id, stream_result| {
            let stream_data = match stream_result {
                Ok(data) => data,
                Err(err) => {
                    eprintln!("an error occurred on stream {:?}: {}", stream_id, err);
                    return;
                }
            };

            let new_outputs = speaker_receiver.try_iter();
            // Append the newest audio to the back, so it's the last read.
            for snippet in new_outputs {
                master_buffer.extend(snippet);
            }

            match stream_data {
                StreamData::Output {
                    buffer: UnknownTypeOutputBuffer::F32(mut buffer),
                } => {
                    // Fill the new audio data in back-to-front
                    for sample in buffer.iter_mut() {
                        *sample = match master_buffer.pop_front() {
                            Some(s) => s,
                            None => 0.0,
                        };
                    }
                }
                _ => panic!("unknown speaker stream data type ;("),
            }
        });
    });

    // === MICROPHONE ===

    let audio_in = audio_host
        .default_input_device()
        .expect("no audio input device available");
    println!("audio in: {}", audio_in.name().unwrap());
    let mic_format = audio_in
        .supported_input_formats()
        .unwrap()
        .next()
        .expect("microphone supports no inputs?")
        .with_max_sample_rate();
    // Start the input stream
    let mic_event_loop = audio_host.event_loop();
    let mic_stream_id = mic_event_loop
        .build_input_stream(&audio_in, &mic_format)
        .expect("The mic's format wasn't supported?");
    mic_event_loop
        .play_stream(mic_stream_id)
        .expect("failed to play mic stream");

    // Create the channel
    let (mic_sender, mic_reciever) = mpsc::channel();

    // Spin up the thread
    let mic_handle = thread::spawn(move || {
        // Start the event loop going!
        mic_event_loop.run(move |stream_id, stream_result| {
            // This gets called many many times
            let stream_data = match stream_result {
                Ok(data) => data,
                Err(err) => {
                    eprintln!("an error occurred on stream {:?}: {}", stream_id, err);
                    return;
                }
            };

            let raw_buffer = match stream_data {
                StreamData::Input {
                    buffer: UnknownTypeInputBuffer::F32(buffer),
                } => buffer,
                _ => panic!("Unknown mic stream data type ;("),
            };

            // Send off the converted audio
            mic_sender
                .send(raw_buffer.iter().map(|sample| *sample).collect())
                .unwrap();
        });
    });

    // and scene

    (
        (speaker_sender, speaker_handle, speaker_format.sample_rate.0),
        (mic_reciever, mic_handle, mic_format.sample_rate.0),
    )
}
