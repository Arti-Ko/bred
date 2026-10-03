//! Opus 1.6: кодировщик и декодер со всем, чего нет в WebCodecs.
//!
//! Кодировщик — безопасная обёртка `opusic-c`. Декодер — свой, поверх сырого
//! libopus: обёртка умеет восстанавливать по глубокой избыточности (DRED) только
//! один кадр перед пакетом, а при обрыве через ретранслятор теряется пачка.

use anyhow::{anyhow, Result};
use opusic_c::{sys, Application, Bitrate, Channels, Encoder, InbandFec, SampleRate, Signal};
use std::ffi::c_int;

/// Частота дискретизации всего звука в звонке.
pub const RATE: u32 = 48_000;
/// Двадцать миллисекунд — родной кадр Opus и шаг всего движка.
pub const FRAME: usize = 960;
/// Самый длинный пакет Opus — сто двадцать миллисекунд.
const MAX_PACKET_SAMPLES: usize = FRAME * 6;
/// Потолок закодированного кадра. Opus сам укладывается в разы меньше.
const MAX_PACKET_BYTES: usize = 4000;
/// Сколько прошлого звука несёт каждый пакет голоса, в десятках миллисекунд.
///
/// Двести миллисекунд — типичный провал при перескоке через ретранслятор.
/// Избыточность едет на крошечном битрейте: это не копия звука, а описание
/// речи, по которому нейросеть декодера восстанавливает пропущенное.
const DRED_UNITS: u8 = 20;

/// Чем кодируем: у голоса и у музыки разные потребности.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Голос: 64 кбит/с, как у Дискорда, с глубокой избыточностью.
    Voice,
    /// Звук демонстрации экрана: всё что угодно, от ролика до игры.
    Screen,
    /// Общий плеер: стерео, 128 кбит/с, без избыточности — музыка терпит
    /// буфер побольше и теряет меньше.
    Music,
}

impl Profile {
    pub fn channels(self) -> usize {
        match self {
            Profile::Music => 2,
            Profile::Voice | Profile::Screen => 1,
        }
    }

    fn bitrate(self) -> u32 {
        match self {
            Profile::Voice => 64_000,
            Profile::Screen => 96_000,
            Profile::Music => 128_000,
        }
    }
}

/// Кодировщик одной исходящей дорожки.
pub struct VoiceEncoder {
    inner: Encoder,
    profile: Profile,
    out: Vec<u8>,
}

// SAFETY: состояние кодировщика — обычная память libopus без привязки к потоку;
// доступ к нему всегда идёт через `&mut`, то есть из одного потока за раз.
unsafe impl Send for VoiceEncoder {}

impl VoiceEncoder {
    pub fn new(profile: Profile) -> Result<Self> {
        let channels = if profile.channels() == 2 {
            Channels::Stereo
        } else {
            Channels::Mono
        };
        let app = match profile {
            Profile::Voice => Application::Voip,
            Profile::Screen | Profile::Music => Application::Audio,
        };
        let mut inner = Encoder::new(channels, SampleRate::Hz48000, app)
            .map_err(|err| anyhow!("кодировщик Opus не создан: {err:?}"))?;

        fn fail(what: &'static str) -> impl Fn(opusic_c::ErrorCode) -> anyhow::Error {
            move |err| anyhow!("Opus: не удалось задать {what}: {err:?}")
        }
        inner
            .set_bitrate(Bitrate::Value(profile.bitrate()))
            .map_err(fail("битрейт"))?;
        inner.set_vbr(true).map_err(fail("переменный битрейт"))?;
        inner.set_complexity(10).map_err(fail("сложность"))?;
        match profile {
            Profile::Voice => {
                inner
                    .set_signal(Signal::Voice)
                    .map_err(fail("тип сигнала"))?;
                // Глубокая избыточность заменяет встроенную: встроенная
                // восстанавливает один пропавший кадр, эта — пачку. Объём
                // избыточности кодировщик считает от ожидаемых потерь, поэтому
                // ноль здесь означал бы «избыточность не нужна».
                inner.set_inband_fec(InbandFec::Off).map_err(fail("FEC"))?;
                inner
                    .set_packet_loss(10)
                    .map_err(fail("ожидаемые потери"))?;
                inner
                    .set_dred_duration(DRED_UNITS)
                    .map_err(fail("глубокую избыточность"))?;
            }
            Profile::Screen => {
                inner
                    .set_signal(Signal::Auto)
                    .map_err(fail("тип сигнала"))?;
                inner
                    .set_inband_fec(InbandFec::Mode2)
                    .map_err(fail("FEC"))?;
                inner.set_packet_loss(5).map_err(fail("ожидаемые потери"))?;
            }
            Profile::Music => {
                inner
                    .set_signal(Signal::Music)
                    .map_err(fail("тип сигнала"))?;
            }
        }
        // Молчание кодируем как молчание, а не выключаем передачу: приёмник
        // по непрерывному потоку видит и потери, и живость собеседника.
        inner.set_dtx(false).map_err(fail("DTX"))?;

        Ok(Self {
            inner,
            profile,
            out: vec![0u8; MAX_PACKET_BYTES],
        })
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    /// Закодировать ровно один кадр: `FRAME` отсчётов на канал, чередующихся.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        if pcm.len() != FRAME * self.profile.channels() {
            return Err(anyhow!("кадр должен быть ровно двадцать миллисекунд"));
        }
        let size = self
            .inner
            .encode_float_to_slice(pcm, &mut self.out)
            .map_err(|err| anyhow!("Opus не закодировал кадр: {err:?}"))?;
        Ok(self.out[..size].to_vec())
    }
}

/// Декодер одного входящего потока: обычное декодирование, восстановление по
/// избыточности и маскирование пропусков.
pub struct VoiceDecoder {
    decoder: *mut sys::OpusDecoder,
    dred_decoder: *mut sys::OpusDREDDecoder,
    dred: *mut sys::OpusDRED,
    channels: usize,
}

// SAFETY: все три указателя принадлежат только этому значению, создаются и
// освобождаются им же, а libopus не держит никакой привязки к потоку. Доступ —
// только через `&mut self`, то есть из одного потока за раз.
unsafe impl Send for VoiceDecoder {}

impl VoiceDecoder {
    pub fn new(channels: usize) -> Result<Self> {
        let channels = channels.clamp(1, 2);
        let mut error: c_int = 0;
        // SAFETY: аргументы в допустимых пределах, указатель ошибки живой.
        let decoder =
            unsafe { sys::opus_decoder_create(RATE as i32, channels as c_int, &mut error) };
        if decoder.is_null() || error != sys::OPUS_OK {
            return Err(anyhow!("декодер Opus не создан: {error}"));
        }
        // Сложность пять и выше включает нейросетевое маскирование пропусков
        // (deep PLC): вместо «металлического» повтора — правдоподобная речь.
        // SAFETY: `decoder` только что создан и не освобождён.
        unsafe {
            sys::opus_decoder_ctl(decoder, sys::OPUS_SET_COMPLEXITY_REQUEST, 10 as c_int);
        }

        // Глубокая избыточность — необязательное улучшение. Не поднялась —
        // работаем на обычном маскировании, но со звуком.
        // SAFETY: указатель ошибки живой; результат проверяется на null.
        let dred_decoder = unsafe { sys::opus_dred_decoder_create(&mut error) };
        // SAFETY: то же.
        let dred = unsafe { sys::opus_dred_alloc(&mut error) };
        if dred_decoder.is_null() || dred.is_null() {
            tracing::debug!("глубокая избыточность недоступна, остаётся обычное маскирование");
        }

        Ok(Self {
            decoder,
            dred_decoder,
            dred,
            channels,
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Сколько отсчётов на канал в пакете. Ошибка — пакет испорчен.
    pub fn samples_in(&self, packet: &[u8]) -> Option<usize> {
        if packet.is_empty() {
            return None;
        }
        // SAFETY: указатель и длина берутся из живого среза.
        let n = unsafe {
            sys::opus_packet_get_nb_samples(packet.as_ptr(), packet.len() as i32, RATE as i32)
        };
        (n > 0).then_some(n as usize)
    }

    /// Обычное декодирование. Возвращает отсчёты, чередующиеся по каналам.
    pub fn decode(&mut self, packet: &[u8]) -> Option<Vec<f32>> {
        let frames = self.samples_in(packet)?.min(MAX_PACKET_SAMPLES);
        let mut out = vec![0f32; frames * self.channels];
        // SAFETY: декодер жив; буфер вмещает `frames` отсчётов на каждый канал.
        let n = unsafe {
            sys::opus_decode_float(
                self.decoder,
                packet.as_ptr(),
                packet.len() as i32,
                out.as_mut_ptr(),
                frames as c_int,
                0,
            )
        };
        (n > 0).then(|| {
            out.truncate(n as usize * self.channels);
            out
        })
    }

    /// Восстановить кадр, стоявший прямо перед `next`, по встроенной
    /// избыточности (FEC) этого пакета. Пакетам без неё libopus ответит
    /// маскированием — тоже лучше тишины.
    pub fn recover_fec(&mut self, next: &[u8]) -> Vec<f32> {
        let mut out = vec![0f32; FRAME * self.channels];
        // SAFETY: декодер жив; буфер ровно на один кадр.
        let n = unsafe {
            sys::opus_decode_float(
                self.decoder,
                next.as_ptr(),
                next.len() as i32,
                out.as_mut_ptr(),
                FRAME as c_int,
                1,
            )
        };
        if n <= 0 {
            return self.conceal();
        }
        out
    }

    /// Сколько прошлого звука можно восстановить из пакета, в отсчётах.
    /// Ноль — избыточности в пакете нет.
    pub fn dred_span(&mut self, packet: &[u8]) -> usize {
        if self.dred_decoder.is_null() || self.dred.is_null() || packet.is_empty() {
            return 0;
        }
        let mut end: c_int = 0;
        // SAFETY: все указатели живы; libopus читает пакет в пределах `len`.
        let available = unsafe {
            sys::opus_dred_parse(
                self.dred_decoder,
                self.dred,
                packet.as_ptr(),
                packet.len() as i32,
                RATE as i32,
                RATE as i32,
                &mut end,
                0,
            )
        };
        available.max(0) as usize
    }

    /// Восстановить кадр, стоявший `back` кадров до пакета, разобранного
    /// последним вызовом `dred_span`.
    pub fn recover_dred(&mut self, back: usize) -> Option<Vec<f32>> {
        if self.dred.is_null() {
            return None;
        }
        let mut out = vec![0f32; FRAME * self.channels];
        // SAFETY: декодер и разобранная избыточность живы; буфер на один кадр.
        let n = unsafe {
            sys::opus_decoder_dred_decode_float(
                self.decoder,
                self.dred,
                (back * FRAME) as i32,
                out.as_mut_ptr(),
                FRAME as i32,
            )
        };
        (n > 0).then_some(out)
    }

    /// Замаскировать пропавший кадр: декодер сам сочиняет правдоподобное
    /// продолжение, а не молчит — молчание посреди слова слышно сильнее.
    pub fn conceal(&mut self) -> Vec<f32> {
        let mut out = vec![0f32; FRAME * self.channels];
        // SAFETY: пустой пакет — штатный способ попросить маскирование.
        let n = unsafe {
            sys::opus_decode_float(
                self.decoder,
                std::ptr::null(),
                0,
                out.as_mut_ptr(),
                FRAME as c_int,
                0,
            )
        };
        if n <= 0 {
            out.fill(0.0);
        }
        out
    }

    /// Забыть историю: собеседник перезашёл, прежнее состояние декодера ему чужое.
    pub fn reset(&mut self) {
        // SAFETY: декодер жив.
        unsafe {
            sys::opus_decoder_ctl(self.decoder, sys::OPUS_RESET_STATE);
        }
    }
}

impl Drop for VoiceDecoder {
    fn drop(&mut self) {
        // SAFETY: каждый указатель создан ровно этим значением и освобождается
        // один раз; null пропускаем.
        unsafe {
            if !self.dred.is_null() {
                sys::opus_dred_free(self.dred);
            }
            if !self.dred_decoder.is_null() {
                sys::opus_dred_decoder_destroy(self.dred_decoder);
            }
            sys::opus_decoder_destroy(self.decoder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тон с огибающей речи: чистый синус кодек сжимает нечестно хорошо.
    fn speechlike(frames: usize) -> Vec<f32> {
        (0..frames * FRAME)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let envelope = 0.5 + 0.5 * (t * 3.0 * std::f32::consts::TAU).sin();
                0.3 * envelope
                    * ((t * 180.0 * std::f32::consts::TAU).sin()
                        + 0.4 * (t * 720.0 * std::f32::consts::TAU).sin())
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    #[test]
    fn voice_round_trips_through_opus() {
        let mut encoder = VoiceEncoder::new(Profile::Voice).unwrap();
        let mut decoder = VoiceDecoder::new(1).unwrap();
        let pcm = speechlike(50);
        let mut energy = 0.0;
        for frame in pcm.chunks(FRAME) {
            let packet = encoder.encode(frame).unwrap();
            assert!(
                packet.len() < 600,
                "голосовой кадр раздулся: {}",
                packet.len()
            );
            let out = decoder.decode(&packet).expect("кадр декодируется");
            assert_eq!(out.len(), FRAME);
            energy += rms(&out);
        }
        assert!(
            energy > 1.0,
            "после декодирования должна остаться речь, а не тишина"
        );
    }

    #[test]
    fn lost_frame_is_concealed_not_silenced() {
        let mut encoder = VoiceEncoder::new(Profile::Voice).unwrap();
        let mut decoder = VoiceDecoder::new(1).unwrap();
        let pcm = speechlike(20);
        let packets: Vec<Vec<u8>> = pcm
            .chunks(FRAME)
            .map(|f| encoder.encode(f).unwrap())
            .collect();
        for packet in &packets[..10] {
            decoder.decode(packet).unwrap();
        }
        let concealed = decoder.conceal();
        assert_eq!(concealed.len(), FRAME);
        assert!(
            rms(&concealed) > 0.001,
            "пропуск посреди речи маскируется продолжением, а не дырой"
        );
    }

    #[test]
    fn burst_loss_is_recovered_from_deep_redundancy() {
        let mut encoder = VoiceEncoder::new(Profile::Voice).unwrap();
        let mut decoder = VoiceDecoder::new(1).unwrap();
        let pcm = speechlike(60);
        let packets: Vec<Vec<u8>> = pcm
            .chunks(FRAME)
            .map(|f| encoder.encode(f).unwrap())
            .collect();
        for packet in &packets[..40] {
            decoder.decode(packet).unwrap();
        }
        // Три кадра пропали, пришёл четвёртый: в нём описание всех трёх.
        let span = decoder.dred_span(&packets[43]);
        assert!(span >= 3 * FRAME, "избыточности хватает на пачку: {span}");
        for back in (1..=3).rev() {
            let restored = decoder.recover_dred(back).expect("кадр восстановлен");
            assert_eq!(restored.len(), FRAME);
        }
        assert!(
            decoder.decode(&packets[43]).is_some(),
            "после восстановления поток идёт дальше"
        );
    }

    #[test]
    fn music_is_stereo() {
        let mut encoder = VoiceEncoder::new(Profile::Music).unwrap();
        let mut decoder = VoiceDecoder::new(2).unwrap();
        let mono = speechlike(5);
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, -*s]).collect();
        for frame in stereo.chunks(FRAME * 2) {
            let packet = encoder.encode(frame).unwrap();
            let out = decoder.decode(&packet).unwrap();
            assert_eq!(out.len(), FRAME * 2, "два канала на выходе");
        }
    }

    #[test]
    fn mono_packet_decodes_into_stereo_decoder() {
        // Старый ведущий мог отдавать музыку в моно — декодер обязан её понять.
        let mut encoder = VoiceEncoder::new(Profile::Voice).unwrap();
        let mut decoder = VoiceDecoder::new(2).unwrap();
        let packet = encoder.encode(&speechlike(1)).unwrap();
        assert_eq!(decoder.decode(&packet).unwrap().len(), FRAME * 2);
    }

    #[test]
    fn garbage_does_not_crash_the_decoder() {
        let mut decoder = VoiceDecoder::new(1).unwrap();
        assert!(decoder.decode(&[]).is_none());
        let _ = decoder.decode(&[0xff; 7]);
        assert_eq!(decoder.dred_span(&[1, 2, 3]), 0);
        assert_eq!(decoder.conceal().len(), FRAME);
    }
}
