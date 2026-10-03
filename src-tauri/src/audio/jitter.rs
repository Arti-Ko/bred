//! Буфер одного входящего звукового потока.
//!
//! Раньше этим занимался вебвью: кадр декодировался сразу по приходу и ставился
//! в очередь воспроизведения «с запасом». Потерянный кадр просто выпадал — звук
//! сдвигался на двадцать миллисекунд, запас таял, и после пары потерь начиналось
//! бульканье. Восстанавливать потерянное там было нечем: WebCodecs не умеет ни
//! избыточность, ни маскирование.
//!
//! Здесь кадр играется строго в свой черёд, а на месте пропавшего звучит
//! восстановленное: по глубокой избыточности соседнего пакета, по встроенной —
//! или, если нет ничего, правдоподобное продолжение от самого декодера.
//! Задержка подбирается под реальный разброс прихода, растёт сразу и тает
//! медленно, а лишнее накопленное выбрасывается в паузах речи, где этого не
//! слышно.

use std::collections::{BTreeMap, VecDeque};

use super::codec::{VoiceDecoder, FRAME};

/// Длительность кадра, миллисекунды.
const FRAME_MS: f64 = 20.0;
/// Самый короткий запас — два кадра. Меньше не выдерживает даже ровный канал:
/// кадры приходят не в такт с часами воспроизведения.
pub const MIN_TARGET: usize = 2;
/// Самый длинный — двенадцать кадров, четверть секунды. Дальше разговор
/// превращается в рацию: люди начинают перебивать друг друга.
pub const MAX_TARGET: usize = 12;
const START_TARGET: usize = 3;
/// По скольким последним кадрам считаем разброс прихода — две секунды.
const WINDOW: usize = 100;
/// Сколько кадров подряд маскируем, когда приходить перестало совсем. Дальше —
/// тишина и ожидание: сочинять речь за ушедшего человека нельзя.
const MAX_CONCEAL: usize = 5;
/// Разрыв в номерах, после которого поток считается начатым заново:
/// собеседник перезашёл, и счёт кадров у него пошёл с единицы.
const RESTART_GAP: u32 = 250;
/// Потолок очереди — секунда. Защита от собеседника, который прислал пачку.
const MAX_QUEUE: usize = 50;
/// Самый длинный провал, который заполняем восстановленным звуком: четверть
/// секунды. Дальше — перепрыгиваем: сочинять секунды речи за молчавшего
/// человека хуже, чем честная тишина.
const MAX_BRIDGE: usize = 12;
/// Тише этого кадр считается паузой, и выбросить его не слышно.
const SILENCE_RMS: f32 = 0.004;
/// Сколько тиков очередь должна держаться выше нормы, прежде чем мы начнём её
/// сжимать. Полсекунды: короткий всплеск рассасывается сам.
const OVER_TICKS: usize = 25;
/// На сколько кадров выше нормы — уже не всплеск, а затор: режем сразу.
const FLOOD: usize = 8;
/// Сколько тиков подряд расчёт должен звать вниз, чтобы мы и правда сократили
/// запас: две секунды. Поспешное снижение возвращает бульканье.
const CALM_TICKS: usize = 100;

/// То, что нужно буферу от декодера. Через типаж — чтобы проверять логику
/// буфера без настоящего Opus.
pub trait Codec {
    fn channels(&self) -> usize;
    fn decode(&mut self, packet: &[u8]) -> Option<Vec<f32>>;
    fn dred_span(&mut self, packet: &[u8]) -> usize;
    fn recover_dred(&mut self, back: usize) -> Option<Vec<f32>>;
    fn recover_fec(&mut self, next: &[u8]) -> Vec<f32>;
    fn conceal(&mut self) -> Vec<f32>;
    fn reset(&mut self);
}

impl Codec for VoiceDecoder {
    fn channels(&self) -> usize {
        VoiceDecoder::channels(self)
    }
    fn decode(&mut self, packet: &[u8]) -> Option<Vec<f32>> {
        VoiceDecoder::decode(self, packet)
    }
    fn dred_span(&mut self, packet: &[u8]) -> usize {
        VoiceDecoder::dred_span(self, packet)
    }
    fn recover_dred(&mut self, back: usize) -> Option<Vec<f32>> {
        VoiceDecoder::recover_dred(self, back)
    }
    fn recover_fec(&mut self, next: &[u8]) -> Vec<f32> {
        VoiceDecoder::recover_fec(self, next)
    }
    fn conceal(&mut self) -> Vec<f32> {
        VoiceDecoder::conceal(self)
    }
    fn reset(&mut self) {
        VoiceDecoder::reset(self)
    }
}

/// Что происходило с потоком — для журнала и для индикатора качества.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub played: u64,
    /// Восстановлено по избыточности — человек этого не услышал.
    pub recovered: u64,
    /// Замаскировано продолжением — слышно как лёгкая смазанность.
    pub concealed: u64,
    /// Опоздало к своему черёду.
    pub late: u64,
    /// Выброшено, чтобы не копить задержку.
    pub skipped: u64,
}

pub struct Jitter<C: Codec> {
    codec: C,
    packets: BTreeMap<u32, Vec<u8>>,
    /// Какой кадр играть следующим. `None` — набираем запас.
    next: Option<u32>,
    target: usize,
    /// Относительная задержка последних кадров: время прихода минус их место
    /// в потоке. Её разброс и есть дрожание канала.
    transit: VecDeque<f64>,
    /// Сколько кадров подряд маскируем без единого пришедшего после.
    starved: usize,
    over: usize,
    calm: usize,
    /// Для какого пакета разобрана избыточность — повторно не разбираем.
    dred_for: Option<(u32, usize)>,
    stats: Stats,
}

impl<C: Codec> Jitter<C> {
    pub fn new(codec: C) -> Self {
        Self {
            codec,
            packets: BTreeMap::new(),
            next: None,
            target: START_TARGET,
            transit: VecDeque::with_capacity(WINDOW),
            starved: 0,
            over: 0,
            calm: 0,
            dred_for: None,
            stats: Stats::default(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn target(&self) -> usize {
        self.target
    }

    pub fn channels(&self) -> usize {
        self.codec.channels()
    }

    /// Играет ли поток прямо сейчас (а не набирает запас).
    pub fn playing(&self) -> bool {
        self.next.is_some()
    }

    /// Пришёл кадр. `arrived_ms` — момент прихода по нашим монотонным часам.
    pub fn push(&mut self, seq: u32, data: Vec<u8>, arrived_ms: f64) {
        // Пока набираем запас, в очереди могли залежаться кадры прошлой жизни
        // потока: человек замолчал, перезашёл — и счёт у него другой.
        if self.next.is_none() {
            if let Some((&last, _)) = self.packets.last_key_value() {
                if seq.abs_diff(last) > RESTART_GAP {
                    self.packets.clear();
                    self.transit.clear();
                }
            }
        }
        if let Some(next) = self.next {
            let behind = next.wrapping_sub(seq);
            let ahead = seq.wrapping_sub(next);
            if behind > 0 && behind < u32::MAX / 2 {
                if behind > RESTART_GAP {
                    self.restart();
                } else {
                    // Опоздал к своему черёду: играть его уже поздно, а запас,
                    // который этого не выдержал, явно мал.
                    self.stats.late += 1;
                    self.raise();
                    return;
                }
            } else if ahead > RESTART_GAP && ahead < u32::MAX / 2 {
                self.restart();
            }
        }

        self.observe(seq, arrived_ms);
        self.packets.entry(seq).or_insert(data);
        while self.packets.len() > MAX_QUEUE {
            self.packets.pop_first();
            self.stats.skipped += 1;
        }
    }

    /// Поток начался заново: всё прежнее относится к прошлой жизни.
    fn restart(&mut self) {
        self.packets.clear();
        self.next = None;
        self.transit.clear();
        self.starved = 0;
        self.over = 0;
        self.dred_for = None;
        self.codec.reset();
    }

    fn raise(&mut self) {
        self.target = (self.target + 1).min(MAX_TARGET);
        self.calm = 0;
    }

    /// Учесть приход для оценки дрожания.
    fn observe(&mut self, seq: u32, arrived_ms: f64) {
        let transit = arrived_ms - seq as f64 * FRAME_MS;
        if self.transit.len() == WINDOW {
            self.transit.pop_front();
        }
        self.transit.push_back(transit);
        if self.transit.len() < 8 {
            return;
        }

        // Разброс — от самого быстрого кадра окна: тот прошёл без задержек, и
        // всё сверх его времени — это и есть то, что надо пережидать. Окно
        // скользит, поэтому и расхождение часов двух машин его не сбивает.
        let fastest = self.transit.iter().copied().fold(f64::INFINITY, f64::min);
        let mut spread: Vec<f64> = self.transit.iter().map(|t| t - fastest).collect();
        spread.sort_by(|a, b| a.total_cmp(b));
        let p95 = spread[(spread.len() * 95 / 100).min(spread.len() - 1)];
        let wanted = ((p95 / FRAME_MS).ceil() as usize + 1).clamp(MIN_TARGET, MAX_TARGET);

        if wanted > self.target {
            // Вверх — сразу: недобор слышно немедленно.
            self.target = wanted;
            self.calm = 0;
        } else if wanted < self.target {
            // Вниз — медленно и по шагу.
            self.calm += 1;
            if self.calm >= CALM_TICKS {
                self.target -= 1;
                self.calm = 0;
            }
        } else {
            self.calm = 0;
        }
    }

    /// Сколько кадров лежит от текущего места и дальше.
    fn queued(&self, from: u32) -> usize {
        self.packets.range(from..).count()
    }

    /// Следующий кадр к воспроизведению: `FRAME` отсчётов на канал.
    /// `None` — играть нечего (набираем запас или собеседник замолчал).
    pub fn pull(&mut self) -> Option<Vec<f32>> {
        let next = match self.next {
            Some(next) => next,
            None => {
                // Набираем запас: первый кадр играем, когда за ним стоит
                // столько, сколько требует нынешнее дрожание канала.
                let first = *self.packets.keys().next()?;
                if self.packets.len() < self.target {
                    return None;
                }
                self.next = Some(first);
                first
            }
        };

        self.trim_flood(next);

        if let Some(packet) = self.packets.remove(&next) {
            self.next = Some(next.wrapping_add(1));
            self.starved = 0;
            let mut out = self.decode_or_conceal(&packet);
            out = self.maybe_skip(out);
            self.stats.played += 1;
            return Some(self.fit(out));
        }

        // Своего кадра нет, но есть более поздние — значит, он потерялся.
        if let Some((&later, _)) = self.packets.range(next.wrapping_add(1)..).next() {
            let back = later.wrapping_sub(next) as usize;
            if back > MAX_BRIDGE {
                // Провал длиннее четверти секунды — человек молчал или канал
                // лежал. Перепрыгиваем к тому, что пришло, а не сочиняем.
                let packet = self.packets.remove(&later).unwrap_or_default();
                self.next = Some(later.wrapping_add(1));
                self.starved = 0;
                self.dred_for = None;
                self.stats.skipped += back as u64;
                let out = self.decode_or_conceal(&packet);
                self.stats.played += 1;
                return Some(self.fit(out));
            }
            let out = self.restore(later, back);
            self.next = Some(next.wrapping_add(1));
            self.starved = 0;
            return Some(self.fit(out));
        }

        // Не пришло ничего: канал провалился или собеседник ушёл. Немного
        // маскируем — короткий провал так не слышен, — а дальше ждём заново.
        self.starved += 1;
        if self.starved == 1 {
            self.raise();
        }
        if self.starved > MAX_CONCEAL {
            // Поток кончился. Всё, что о нём помнили, относится к прошлому:
            // разброс прихода посчитан по старым номерам (собеседник мог
            // перезайти, и счёт у него пошёл с единицы — тогда старое окно
            // задрало бы запас до потолка на десятки секунд), а состояние
            // декодера — по оборванной речи.
            self.next = None;
            self.starved = 0;
            self.dred_for = None;
            self.transit.clear();
            self.codec.reset();
            return None;
        }
        self.next = Some(next.wrapping_add(1));
        self.stats.concealed += 1;
        let out = self.codec.conceal();
        Some(self.fit(out))
    }

    /// Восстановить кадр, стоящий `back` кадров до пакета `later`.
    fn restore(&mut self, later: u32, back: usize) -> Vec<f32> {
        let span = match self.dred_for {
            Some((seq, span)) if seq == later => span,
            _ => {
                let span = self
                    .packets
                    .get(&later)
                    .map(|p| self.codec.dred_span(p))
                    .unwrap_or(0);
                self.dred_for = Some((later, span));
                span
            }
        };
        if span >= back * FRAME {
            if let Some(out) = self.codec.recover_dred(back) {
                self.stats.recovered += 1;
                return out;
            }
        }
        if back == 1 {
            if let Some(packet) = self.packets.get(&later) {
                let out = self.codec.recover_fec(packet);
                self.stats.recovered += 1;
                return out;
            }
        }
        self.stats.concealed += 1;
        self.codec.conceal()
    }

    fn decode_or_conceal(&mut self, packet: &[u8]) -> Vec<f32> {
        match self.codec.decode(packet) {
            Some(out) => out,
            None => {
                self.stats.concealed += 1;
                self.codec.conceal()
            }
        }
    }

    /// Очередь давно выше нормы, а этот кадр — пауза: выбрасываем его и сразу
    /// играем следующий. Так задержка уходит без единого слышного шва.
    fn maybe_skip(&mut self, out: Vec<f32>) -> Vec<f32> {
        let Some(next) = self.next else {
            return out;
        };
        if self.queued(next) + 1 > self.target + 1 {
            self.over += 1;
        } else {
            self.over = 0;
        }
        if self.over < OVER_TICKS || rms(&out) >= SILENCE_RMS {
            return out;
        }
        let Some(packet) = self.packets.remove(&next) else {
            return out;
        };
        self.next = Some(next.wrapping_add(1));
        self.over = 0;
        self.stats.skipped += 1;
        self.decode_or_conceal(&packet)
    }

    /// Пришла пачка после затора — догонять её бессмысленно, отставание
    /// останется навсегда. Режем до нормы, даже ценой одного шва.
    fn trim_flood(&mut self, next: u32) {
        let queued = self.queued(next);
        if queued <= self.target + FLOOD {
            return;
        }
        let excess = queued - (self.target + 1);
        let doomed: Vec<u32> = self
            .packets
            .range(next..)
            .take(excess)
            .map(|(s, _)| *s)
            .collect();
        if let Some(last) = doomed.last() {
            self.next = Some(last.wrapping_add(1));
        }
        for seq in doomed {
            self.packets.remove(&seq);
            self.stats.skipped += 1;
        }
        self.dred_for = None;
        self.over = 0;
    }

    /// Ровно один кадр на выходе: пакеты длиннее двадцати миллисекунд у нас не
    /// ходят, а короткий выход декодера на ошибке добиваем тишиной.
    fn fit(&self, mut out: Vec<f32>) -> Vec<f32> {
        out.resize(FRAME * self.codec.channels(), 0.0);
        out
    }
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Декодер-заглушка: «звук» кадра — его номер, по выходу видно, что играло.
    #[derive(Default)]
    struct Fake {
        dred: usize,
        recovered: Vec<usize>,
        concealed: usize,
        resets: usize,
    }

    impl Codec for Fake {
        fn channels(&self) -> usize {
            1
        }
        fn decode(&mut self, packet: &[u8]) -> Option<Vec<f32>> {
            Some(vec![packet[0] as f32; FRAME])
        }
        fn dred_span(&mut self, _packet: &[u8]) -> usize {
            self.dred
        }
        fn recover_dred(&mut self, back: usize) -> Option<Vec<f32>> {
            self.recovered.push(back);
            Some(vec![-1.0; FRAME])
        }
        fn recover_fec(&mut self, _next: &[u8]) -> Vec<f32> {
            self.recovered.push(0);
            vec![-2.0; FRAME]
        }
        fn conceal(&mut self) -> Vec<f32> {
            self.concealed += 1;
            vec![-3.0; FRAME]
        }
        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    fn label(out: Option<Vec<f32>>) -> Option<f32> {
        out.map(|o| o[0])
    }

    /// Ровный канал: кадр приходит каждые двадцать миллисекунд.
    fn steady(jitter: &mut Jitter<Fake>, seqs: std::ops::Range<u32>) {
        for seq in seqs {
            jitter.push(seq, vec![seq as u8], seq as f64 * FRAME_MS);
        }
    }

    #[test]
    fn waits_for_its_cushion_then_plays_in_order() {
        let mut j = Jitter::new(Fake::default());
        j.push(1, vec![1], 20.0);
        assert_eq!(j.pull(), None, "один кадр — ещё не запас");
        steady(&mut j, 2..6);
        assert_eq!(label(j.pull()), Some(1.0));
        assert_eq!(label(j.pull()), Some(2.0));
        assert_eq!(label(j.pull()), Some(3.0));
    }

    #[test]
    fn lost_burst_is_rebuilt_from_deep_redundancy() {
        let mut j = Jitter::new(Fake {
            dred: FRAME * 10,
            ..Fake::default()
        });
        steady(&mut j, 1..4);
        // Кадры 4, 5 и 6 пропали, пришёл седьмой.
        j.push(7, vec![7], 140.0);
        j.push(8, vec![8], 160.0);
        for expected in [1.0, 2.0, 3.0] {
            assert_eq!(label(j.pull()), Some(expected));
        }
        for _ in 0..3 {
            assert_eq!(label(j.pull()), Some(-1.0), "пропавший кадр восстановлен");
        }
        assert_eq!(
            j.codec.recovered,
            vec![3, 2, 1],
            "по порядку, от дальнего к ближнему"
        );
        assert_eq!(label(j.pull()), Some(7.0), "и дальше поток идёт как шёл");
        assert_eq!(j.stats().recovered, 3);
    }

    #[test]
    fn single_loss_without_redundancy_uses_fec() {
        let mut j = Jitter::new(Fake::default());
        steady(&mut j, 1..4);
        j.push(5, vec![5], 100.0);
        for _ in 0..3 {
            j.pull();
        }
        assert_eq!(
            label(j.pull()),
            Some(-2.0),
            "встроенная избыточность соседнего пакета"
        );
        assert_eq!(label(j.pull()), Some(5.0));
    }

    #[test]
    fn silence_after_the_stream_is_masked_briefly_then_stops() {
        let mut j = Jitter::new(Fake::default());
        steady(&mut j, 1..4);
        for _ in 0..3 {
            j.pull();
        }
        let mut concealed = 0;
        while let Some(out) = j.pull() {
            assert_eq!(out[0], -3.0);
            concealed += 1;
            assert!(
                concealed <= MAX_CONCEAL,
                "сочинять речь за ушедшего бесконечно нельзя"
            );
        }
        assert_eq!(concealed, MAX_CONCEAL);
        assert!(!j.playing(), "после провала снова набираем запас");
    }

    #[test]
    fn rejoin_after_silence_does_not_inherit_old_jitter() {
        let mut j = Jitter::new(Fake::default());
        steady(&mut j, 5000..5020);
        let resets = j.codec.resets;
        while j.pull().is_some() {}
        // Перезашёл через полминуты: номера с единицы, время — далеко вперёд.
        for seq in 1..20u32 {
            j.push(seq, vec![seq as u8], 130_000.0 + seq as f64 * FRAME_MS);
        }
        assert!(
            j.target() <= START_TARGET + 1,
            "запас не должен взлететь: {}",
            j.target()
        );
        assert!(
            j.codec.resets > resets,
            "после обрыва потока декодер начинает с чистого листа"
        );
    }

    #[test]
    fn late_packet_is_dropped_and_cushion_grows() {
        let mut j = Jitter::new(Fake::default());
        steady(&mut j, 1..5);
        let before = j.target();
        for _ in 0..4 {
            j.pull();
        }
        j.push(2, vec![2], 200.0);
        assert_eq!(j.stats().late, 1);
        assert!(j.target() > before, "опоздание — знак, что запас мал");
    }

    #[test]
    fn rejoined_sender_starts_a_fresh_stream() {
        let mut j = Jitter::new(Fake::default());
        steady(&mut j, 1000..1004);
        j.pull();
        // Человек перезашёл: счёт снова с единицы.
        steady(&mut j, 1..5);
        assert_eq!(j.codec.resets, 1, "прежнее состояние декодера ему чужое");
        assert_eq!(label(j.pull()), Some(1.0));
    }

    #[test]
    fn jittery_channel_grows_the_cushion() {
        let mut j = Jitter::new(Fake::default());
        // Каждый пятый кадр задерживается на сто миллисекунд.
        for seq in 1..60u32 {
            let delay = if seq % 5 == 0 { 100.0 } else { 0.0 };
            j.push(seq, vec![seq as u8], seq as f64 * FRAME_MS + delay);
        }
        assert!(
            j.target() >= 6,
            "запас обязан покрыть дрожание: {}",
            j.target()
        );
    }

    #[test]
    fn long_silence_is_jumped_over_not_invented() {
        let mut j = Jitter::new(Fake::default());
        steady(&mut j, 1..4);
        for _ in 0..3 {
            j.pull();
        }
        // Человек замолчал на две секунды и заговорил снова.
        steady(&mut j, 104..110);
        let mut invented = 0;
        loop {
            let out = j.pull().expect("звук идёт");
            if out[0] >= 104.0 {
                break;
            }
            invented += 1;
        }
        assert!(
            invented <= MAX_BRIDGE,
            "за молчавшего насочиняли {invented} кадров вместо прыжка"
        );
    }

    #[test]
    fn stale_frames_from_a_previous_life_are_dropped() {
        let mut j = Jitter::new(Fake::default());
        // Два кадра прошлой жизни так и не набрали запас…
        j.push(900, vec![9], 0.0);
        j.push(901, vec![9], 20.0);
        // …а человек перезашёл, и счёт пошёл с единицы.
        steady(&mut j, 1..5);
        assert_eq!(label(j.pull()), Some(1.0));
        for _ in 0..3 {
            j.pull();
        }
        assert!(
            j.packets.range(900..).next().is_none(),
            "кадры прошлой жизни не должны ждать своей очереди"
        );
    }

    #[test]
    fn flood_after_a_stall_is_cut_down() {
        let mut j = Jitter::new(Fake::default());
        steady(&mut j, 1..4);
        j.pull();
        // Затор: сразу тридцать кадров одной пачкой.
        for seq in 4..34u32 {
            j.push(seq, vec![seq as u8], 80.0);
        }
        j.pull();
        assert!(
            j.queued(j.next.unwrap()) <= j.target() + 1,
            "догонять пачку бессмысленно — отставание останется навсегда"
        );
    }

    #[test]
    fn standing_backlog_melts_away_in_pauses() {
        let mut j = Jitter::new(Fake::default());
        // Канал ровный, но воспроизведение стартовало поздно: в очереди восемь
        // кадров при норме в два-три. Кадры — «паузы»: у заглушки номер ноль
        // декодируется в тишину.
        for seq in 1..9u32 {
            j.push(seq, vec![0], seq as f64 * FRAME_MS);
        }
        j.pull();
        let start = j.queued(j.next.unwrap());
        for seq in 9..90u32 {
            j.push(seq, vec![0], seq as f64 * FRAME_MS);
            j.pull();
        }
        assert!(j.stats().skipped > 0, "лишнее в паузах выбрасывается");
        assert!(j.queued(j.next.unwrap()) < start, "очередь сократилась");
    }
}
