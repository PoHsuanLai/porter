use super::*;
use model_provider::{Support, ToolSupport};
use std::collections::BTreeSet;

fn text(context: u32, context_max: Option<u32>, context_min: Option<u32>) -> TextOut {
    TextOut {
        tools: ToolSupport::Absent,
        structured: BTreeSet::new(),
        reasoning: Support::Absent,
        streaming: Support::Present,
        context: Tokens(context),
        max_output: Tokens(4096),
        context_max: context_max.map(Tokens),
        max_output_max: Some(Tokens(16384)),
        context_min: context_min.map(Tokens),
        sampling: None,
    }
}

/// 15000 MiB of weights, 1000 of overhead, and 500 MiB for every thousand tokens of window.
fn vram(per_thousand: u32) -> VramEstimate {
    VramEstimate {
        weights: MiB(15_000),
        kv_per_1k_ctx: MiB(per_thousand),
        overhead: MiB(1_000),
    }
}

/// What happens, the entry's text, kv per thousand, free memory, the person's length, window.
type WindowCase = (&'static str, TextOut, u32, Option<u32>, Option<u32>, u32);

#[test]
fn the_window_is_the_largest_that_fits_cut_to_blocks_and_never_below_what_is_served() {
    let cases: [WindowCase; 10] = [
        (
            "the model's largest caps a card with room to spare",
            text(16_384, Some(65_536), None),
            500,
            Some(100_000),
            None,
            65_536,
        ),
        (
            "free memory limits the window, in whole blocks (48000 tokens fit)",
            text(16_384, Some(65_536), None),
            500,
            Some(40_000),
            None,
            47_104,
        ),
        (
            "too little memory keeps what is served",
            text(16_384, Some(65_536), None),
            500,
            Some(20_000),
            None,
            16_384,
        ),
        (
            "a smallest useful window that fits is a window that fits",
            text(16_384, Some(65_536), Some(32_768)),
            500,
            Some(40_000),
            None,
            47_104,
        ),
        (
            "a smallest useful window that does not fit keeps what is served",
            text(16_384, Some(65_536), Some(32_768)),
            500,
            Some(30_000),
            None,
            16_384,
        ),
        (
            "with no largest stated, what is served is the largest",
            text(16_384, None, None),
            500,
            Some(100_000),
            None,
            16_384,
        ),
        (
            "the person's length wins over free memory",
            text(16_384, Some(65_536), None),
            500,
            Some(20_000),
            Some(24_576),
            24_576,
        ),
        (
            "the person's length does not pass the model's largest",
            text(16_384, Some(65_536), None),
            500,
            Some(100_000),
            Some(200_000),
            65_536,
        ),
        (
            "a memory figure of zero keeps what is served",
            text(16_384, Some(65_536), None),
            0,
            Some(100_000),
            None,
            16_384,
        ),
        (
            "free memory not known keeps what is served",
            text(16_384, Some(65_536), None),
            500,
            None,
            None,
            16_384,
        ),
    ];
    for (what, text, per_thousand, free, own, want) in cases {
        let got = choose_window(&text, &vram(per_thousand), free.map(MiB), own.map(Tokens));
        assert_eq!(got, Tokens(want), "{what}");
    }
}

#[test]
fn a_chosen_window_always_fits_the_memory_it_was_chosen_for() {
    let text = text(16_384, Some(131_072), None);
    for free in (18_000..60_000).step_by(997) {
        let window = choose_window(&text, &vram(500), Some(MiB(free)), None);
        let fits = vram(500).need(window).0 <= free;
        assert!(fits || window == text.context, "{free} MiB: {window:?}");
    }
}

#[test]
fn a_reply_may_be_what_the_model_supports_cut_to_half_the_window_and_to_the_persons_length() {
    let supports = text(16_384, Some(65_536), None);
    let mut plain = text(16_384, Some(65_536), None);
    plain.max_output_max = None;
    let own = |reply| Lengths {
        conversation: None,
        reply: Some(Tokens(reply)),
    };
    let cases: [(&str, &TextOut, Lengths, Option<u32>, u32); 6] = [
        (
            "the model's most, in a large window",
            &supports,
            Lengths::default(),
            Some(65_536),
            16_384,
        ),
        (
            "half a small window, so the prompt has room",
            &supports,
            Lengths::default(),
            Some(16_384),
            8_192,
        ),
        (
            "a window not known bounds nothing",
            &supports,
            Lengths::default(),
            None,
            16_384,
        ),
        (
            "with no most stated, the served reply is the most",
            &plain,
            Lengths::default(),
            Some(65_536),
            4_096,
        ),
        (
            "the person's length is lower",
            &supports,
            own(2_048),
            Some(65_536),
            2_048,
        ),
        (
            "the person's length is never past what the model supports",
            &plain,
            own(12_000),
            Some(65_536),
            4_096,
        ),
    ];
    for (what, text, lengths, window, want) in cases {
        let got = reply_ceiling(text, lengths, window.map(Tokens));
        assert_eq!(got, Tokens(want), "{what}");
    }
}

#[test]
fn an_engine_line_that_takes_a_share_of_the_card_is_fitted_inside_it() {
    let args = |line: &[&str]| -> Vec<EngineArg> {
        line.iter()
            .map(|arg| EngineArg((*arg).to_owned()))
            .collect()
    };
    let card = MiB(96_000);
    let cases: [(&str, Vec<EngineArg>, Option<u32>); 5] = [
        (
            "a flag and its value",
            args(&["--max-num-seqs", "4", "--gpu-memory-utilization", "0.75"]),
            Some(72_000),
        ),
        (
            "written with =",
            args(&["--gpu-memory-utilization=0.5"]),
            Some(48_000),
        ),
        (
            "a line that says nothing",
            args(&["--max-num-seqs", "4"]),
            None,
        ),
        (
            "a share past the whole card",
            args(&["--gpu-memory-utilization", "1.5"]),
            None,
        ),
        (
            "a flag with no value",
            args(&["--gpu-memory-utilization"]),
            None,
        ),
    ];
    for (what, line, want) in cases {
        assert_eq!(share_of(&line, card), want.map(MiB), "{what}");
    }
}
