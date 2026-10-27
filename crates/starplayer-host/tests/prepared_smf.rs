#![cfg(feature = "note-overlap")]
use starplayer::core::{Command, Frame, InstrumentId, PlaybackCapabilities, U0F16};
use starplayer::engine::{Engine, EngineSettings, MidiSource, PlaybackConfiguration, VoiceCapacity, VoiceDomain};
use starplayer::engine::instrument::live::RackScope;
use starplayer::mixer::{FixedPath, StereoI16};
use starplayer::model::{Envelope, EnvelopePoint, InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, NewNoteAction, SampleSpec};
use starplayer::rt::Arc;
fn module(format: ModuleFormat) -> Arc<Module> { timed_module(format,125) }
fn timed_module(format: ModuleFormat,tempo:u16) -> Arc<Module> {
    let mut builder = ModuleBuilder::new(); let sample = builder.add_sample(&[1500;32768],SampleSpec::one_shot("SMF")).unwrap();
    let mut instrument = InstrumentDef::from_sample("native",sample,U0F16::MAX); instrument.note_sample_map.fill(1); for (note,mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
    instrument.volume_envelope = Some(Envelope {points:vec![EnvelopePoint {tick:0,value:64},EnvelopePoint {tick:10,value:0}].into_boxed_slice(),..Envelope::default()}); instrument.new_note_action = NewNoteAction::Continue;
    builder.add_instrument(instrument).unwrap(); let mut header = ModuleHeader::new(format,16); header.flags.linear_slides = true; header.initial_tempo = tempo;
    if format == ModuleFormat::It { header.format_extra = starplayer::it::ItFormatExtra {flags:12,special:0,old_instruments:false,has_midi_configuration:false}.encode(); }
    builder.set_header(header); Arc::new(builder.build().unwrap())
}
fn smf() -> starplayer::midi::smf::Smf {
    let track = [0,0x90,60,127, 3,0x90,67,127, 3,0x90,60,127, 96,0xff,0x2f,0];
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk".to_vec(); bytes.extend_from_slice(&(track.len() as u32).to_be_bytes()); bytes.extend_from_slice(&track); starplayer::midi::smf::parse_smf(&bytes).unwrap()
}
#[test]
fn prepared_smf_native_cadence_and_song_domains_work_without_live_reservations() {
    for format in [ModuleFormat::Xm,ModuleFormat::It] {
        let module = module(format); let mut capabilities = PlaybackCapabilities::empty();
        #[cfg(feature = "note-overlap")] { capabilities |= PlaybackCapabilities::NOTE_OVERLAP; }
        // Test a noninteractive engine even when both capabilities are compiled.
        #[cfg(not(feature = "note-overlap"))] { capabilities |= PlaybackCapabilities::INTERACTIVE_AUDITION; }
        let configuration = PlaybackConfiguration {identity:Default::default(),capabilities,instrument_count:1,capacity:VoiceCapacity {native:256,custom:if cfg!(feature="note-overlap") {4} else {0},live:0,interactive_transport:false}};
        // Audition-only requires interactive layout; the no-L song case is overlap-only.
        #[cfg(not(feature = "note-overlap"))] { let _ = (module,configuration); continue; }
        #[cfg(feature = "note-overlap")]
        {
            let settings = EngineSettings {sample_rate_hz:8000,channel_count:16,..EngineSettings::default()}.with_playback(configuration).unwrap();
            let rack = starplayer::prepare_instrument_rack(&module,8000,settings.voice_capacity,RackScope::Song).unwrap();
            let feed = starplayer::midi::SmfSequencer::prepared_song_at(&smf(),8000,Frame::ZERO);
            let mut engine: Engine<FixedPath,starplayer::dsp::Linear,StereoI16,Arc<Module>> = Engine::with_settings(settings);
            let mut handle = engine.take_control().unwrap(); handle.load_module(Arc::clone(&module)).ok().unwrap(); handle.send(Command::Play).unwrap(); engine.set_source(Box::new(MidiSource::new(feed,rack,8000)));
            engine.restore_selections(Default::default(),true,&[true]);
            let mut output = [0;256]; engine.render(&mut output);
            let voice = engine.channels().foreground(starplayer::core::ChannelId(0)).unwrap();
            assert!(engine.voices().get(voice).unwrap().params.volume.to_bits() > 60000,"1ms polling must not consume the ten-tick envelope");
            let owned = engine.channels().playback().unwrap().get(voice,engine.voices()).unwrap(); assert_eq!(owned.domain,VoiceDomain::Native); assert_eq!(owned.identity().unwrap().instrument,InstrumentId(0));
            engine.render(&mut output); engine.render(&mut output);
            assert!(engine.channels().playback().unwrap().active_count(VoiceDomain::Custom,engine.voices()) > 0,"SMF overlap uses C");
            assert_eq!(engine.channels().playback().unwrap().active_count(VoiceDomain::Live,engine.voices()),0);
        }
    }
}

#[test]
fn prepared_smf_uses_initial_module_tempo_and_native_fractional_model() {
    for format in [ModuleFormat::Xm,ModuleFormat::It] { for rate in [8000,44117] {
        let module=timed_module(format,137);
        let configuration=PlaybackConfiguration {identity:Default::default(),capabilities:PlaybackCapabilities::NOTE_OVERLAP,instrument_count:1,capacity:VoiceCapacity {native:256,custom:4,live:0,interactive_transport:false}};
        let settings=EngineSettings {sample_rate_hz:rate,channel_count:16,..EngineSettings::default()}.with_playback(configuration).unwrap();
        let rack=starplayer::prepare_instrument_rack(&module,rate,settings.voice_capacity,RackScope::Song).unwrap();
        let feed=starplayer::midi::SmfSequencer::prepared_song_at(&smf(),rate,Frame::ZERO);
        let mut engine:Engine<FixedPath,starplayer::dsp::Linear,StereoI16,Arc<Module>>=Engine::with_settings(settings);
        let mut handle=engine.take_control().unwrap();handle.load_module(Arc::clone(&module)).ok().unwrap();handle.send(Command::Play).unwrap();engine.set_source(Box::new(MidiSource::new(feed,rack,rate)));
        let tempo=starplayer::core::quirks::QuirkSelection::FromDialect.resolve(module.header().dialect).tempo_model;
        let mut clock=starplayer::core::FrameClock::new(tempo,rate,Frame::ZERO);let mut ticks=0;
        for _ in 0..100 {
            engine.render(&mut [0;256]);
            while clock.pending_tick_frame()<engine.source_frame() {clock.advance_tick(137,6);ticks+=1;}
            assert_eq!(engine.control_clock().ticks(),ticks,"{format:?} rate{rate}");
            assert_eq!(engine.control_clock().next_synthesised_frame(),Some(clock.pending_tick_frame()));
        }
    } }
}
