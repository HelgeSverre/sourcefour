#!/usr/bin/env python3
"""Sourcefour social spot: the audio and the frame timeline, from one clock.

Synthesizes a 15s techno piece (128 BPM, 32 beats, C minor) whose hits line
up with the wordmark animation, and writes the ffmpeg concat lists that cut
the frame PNGs on the same grid. At 128 fps every 16th note is exactly 15
frames, so the cuts are frame-exact against the audio. The synthesis recipes
are ported from ~/code/rave-c (kick pitch-sweep, 303 saw->SVF bass, noise
hats/clap, sidechain duck, stab delay send) in dependency-free Python.

Timeline (one bar per phase of the gag, then the outro):
  bar 1  Sourcetree, plain groove
  bar 2  "tree" blinks on 8ths, hats go 16ths
  bar 3  "++" lands on the downbeat, blink doubles to 16ths, riser + roll
  bar 4  drop: Sourcefour on the one, four<->4 swaps every beat
  bar 5  swaps double to 8ths, then the mark locks on "Sourcefour"
  bars 6-8  elements drop out one by one and the whole thing fades
"""

import math
import os
import random
import struct
import wave

SR = 44100
BPM = 128.0
BEAT = 60.0 / BPM
BEATS = 32
SECONDS = BEAT * BEATS  # 15.0
N = int(round(SR * SECONDS))

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "out")


def mtof(m):
    return 440.0 * 2.0 ** ((m - 69) / 12.0)


def tc(seconds):
    """Per-sample decay multiplier with the given time constant."""
    return math.exp(-1.0 / (seconds * SR))


class Svf:
    """Chamberlin state-variable filter, one per voice like rave-c."""

    __slots__ = ("lo", "bp")

    def __init__(self):
        self.lo = 0.0
        self.bp = 0.0

    def run(self, x, fc, q):
        f = 2.0 * math.sin(math.pi * min(fc, 7000.0) / SR)
        self.lo += f * self.bp
        hi = x - self.lo - q * self.bp
        self.bp += f * hi
        return self.lo, self.bp, hi


CHORD_A = [60, 63, 67]  # Cm — shows "four"
CHORD_B = [58, 62, 65]  # Bb — shows "4"


def events():
    """(beat, kind, data) for the whole piece."""
    ev = []
    for b in range(BEATS):
        bar = b // 4
        if bar < 7 or b == 28:
            ev.append((b, "kick", None))
        if b % 2 == 1 and bar < 6:
            ev.append((b, "clap", None))
        if bar in (0, 3, 4):
            ev.append((b + 0.5, "hat", 0.030))
        elif bar in (1, 2):
            for s in range(4):
                ev.append((b + s / 4, "hat", 0.030))
        # bass: 16ths around the kick, octave pop on alternate beats,
        # thinning to lone 8ths in bar 7 and out entirely in bar 8
        if bar < 6:
            for s in (1, 2, 3):
                up = s == 2 and b % 2 == 1
                ev.append((b + s / 4, "bass", (36 + (12 if up else 0), up)))
        elif bar == 6:
            ev.append((b + 0.5, "bass", (36, False)))
    # the ++ hit, the drop swaps, the 8th-note swaps, the lock, the goodbye
    ev.append((8, "hat", 0.20))
    swaps = [(12 + i, i) for i in range(4)]
    swaps += [(16 + i / 2, i) for i in range(4)]
    for beat, i in swaps:
        ev.append((beat, "hat", 0.20))
        ev.append((beat, "stab", CHORD_A if i % 2 == 0 else CHORD_B))
    ev.append((18, "hat", 0.20))
    ev.append((18, "stab", CHORD_A))
    ev.append((28, "stab", CHORD_A))
    # snare roll into the drop: 16ths then 32nds, rising
    t = 10.0
    while t < 12.0:
        vel = 0.35 + 0.65 * (t - 10.0) / 2.0
        ev.append((t, "snare", vel))
        t += 0.25 if t < 11.0 else 0.125
    return ev


def synthesize():
    random.seed(4)  # sourcefour
    frand = random.random

    trig = {}
    for beat, kind, data in events():
        trig.setdefault(int(round(beat * BEAT * SR)), []).append((kind, data))

    # decay constants (rave-c's numbers)
    k_pc, k_ac = tc(0.020), tc(0.10)
    b_fc, b_ac = tc(0.16), tc(0.11)
    s_fc, s_ac = tc(0.28), tc(0.30)
    sn_ac = tc(0.055)
    c_burst, c_tail = tc(0.008), tc(0.040)
    duck_c = tc(0.18)

    # voice state
    k_ph = k_pe = k_ae = 0.0
    h_ae, h_dec, h_hp = 0.0, 1.0, 0.0
    c_ae, c_n = 0.0, 10**9
    c_svf = Svf()
    sn_ae, sn_p1, sn_p2 = 0.0, 0.0, 0.0
    sn_svf = Svf()
    b_ph = b_fe = b_ae = 0.0
    b_freq, b_acc = mtof(36), False
    b_svf = Svf()
    st_ph = [0.0, 0.0, 0.0]
    st_freq = [0.0, 0.0, 0.0]
    st_fe = st_ae = 0.0
    st_svf = Svf()
    r_svf = Svf()
    duck = 0.0
    DET = (0.996, 1.0, 1.004)

    DMASK = 16383
    dbuf = [0.0] * (DMASK + 1)
    dpos = 0
    dtime = int(3 * BEAT / 4 * SR)  # dotted-8th echo, fits the 16k buffer

    frames = []
    for n in range(N):
        for kind, data in trig.get(n, ()):
            if kind == "kick":
                k_ph, k_pe, k_ae, duck = 0.0, 1.0, 1.0, 1.0
            elif kind == "hat":
                h_ae, h_dec = 1.0, tc(data)
            elif kind == "clap":
                c_ae, c_n = 1.0, 0
            elif kind == "snare":
                sn_ae = data
            elif kind == "bass":
                midi, accent = data
                b_freq, b_acc = mtof(midi), accent
                b_ph, b_fe, b_ae = 0.0, 1.0, 1.0
            elif kind == "stab":
                st_freq = [mtof(m) for m in data]
                st_fe = st_ae = 1.0

        # kick: sine, exp pitch env 180->48
        k_pe *= k_pc
        k_ae *= k_ac
        k_ph += (48.0 + 132.0 * k_pe) / SR
        kick = math.tanh(math.sin(2 * math.pi * k_ph) * 2.6) * k_ae

        # hat: highpassed noise
        h_ae *= h_dec
        x = frand() * 2 - 1
        h_hp += 0.25 * (x - h_hp)
        hat = (x - h_hp) * h_ae

        # clap: three 10ms-apart bursts + tail
        c_n += 1
        if c_n == int(0.010 * SR) or c_n == int(0.020 * SR):
            c_ae = 1.0
        c_ae *= c_burst if c_n < int(0.030 * SR) else c_tail
        _, bp, _ = c_svf.run(frand() * 2 - 1, 1500.0, 0.7)
        clap = bp * 1.6 * c_ae

        # snare: bandpassed noise + two tones
        sn_ae *= sn_ac
        _, bp, _ = sn_svf.run(frand() * 2 - 1, 1800.0, 0.8)
        sn_p1 = (sn_p1 + 180.0 / SR) % 1.0
        sn_p2 = (sn_p2 + 330.0 / SR) % 1.0
        tones = math.sin(2 * math.pi * sn_p1) + math.sin(2 * math.pi * sn_p2)
        snare = (bp * 1.4 + tones * 0.35 * sn_ae) * sn_ae

        # bass: saw -> lowpass, acid env, accent opens the filter
        b_fe *= b_fc
        b_ae *= b_ac
        b_ph = (b_ph + b_freq / SR) % 1.0
        cut = 140.0 + (900.0 + (2400.0 if b_acc else 0.0)) * b_fe
        lo, _, _ = b_svf.run(b_ph * 2 - 1, cut, 1.1)
        bass = math.tanh(lo * 1.5) * b_ae

        # stab: three detuned saws -> lowpass (the four<->4 voice)
        st_fe *= s_fc
        st_ae *= s_ac
        ssum = 0.0
        if st_freq[0] > 1.0:
            for i in range(3):
                st_ph[i] = (st_ph[i] + st_freq[i] * DET[i] / SR) % 1.0
                ssum += st_ph[i] * 2 - 1
            ssum /= 3.0
        lo, _, _ = st_svf.run(ssum, 700.0 + 4200.0 * st_fe, 0.6)
        stab = lo * st_ae

        # riser: bandpassed noise sweeping up through bar 3
        beat_pos = n / SR / BEAT
        if 8.0 <= beat_pos < 12.0:
            r = (beat_pos - 8.0) / 4.0
            _, bp, _ = r_svf.run(frand() * 2 - 1, 600.0 + 6000.0 * r * r, 0.9)
            riser = bp * 0.5 * r * r
        else:
            riser = 0.0

        # sidechain
        duck *= duck_c
        dbass = 1.0 - 0.75 * duck
        dsynt = 1.0 - 0.60 * duck

        dry = (
            kick * 0.95
            + snare * 0.50
            + hat * 0.24
            + clap * 0.45
            + bass * 0.52 * dbass
            + stab * 0.30 * dsynt
            + riser
        )
        send = stab * 0.45
        dl = dbuf[(dpos - dtime) & DMASK]
        dr = dbuf[(dpos - dtime + 192) & DMASK]
        dbuf[dpos] = send + dl * 0.35
        dpos = (dpos + 1) & DMASK

        left = math.tanh((dry + dl * 0.45) * 0.85)
        right = math.tanh((dry + dr * 0.45) * 0.85)
        frames.append((left, right))
    return frames


def write_wav(frames, path):
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(
            b"".join(
                struct.pack(
                    "<hh",
                    int(max(-1.0, min(1.0, l)) * 32767),
                    int(max(-1.0, min(1.0, r)) * 32767),
                )
                for l, r in frames
            )
        )


def segments():
    """(state, seconds) for the whole piece, on the same beat grid."""
    seg = [(0, 4 * BEAT)]
    seg += [(0 if i % 2 == 0 else 1, BEAT / 2) for i in range(8)]
    seg += [(2 if i % 2 == 0 else 3, BEAT / 4) for i in range(16)]
    seg += [(4, BEAT), (5, BEAT), (4, BEAT), (5, BEAT)]
    seg += [(4, BEAT / 2), (5, BEAT / 2), (4, BEAT / 2), (5, BEAT / 2)]
    # the lock: Sourcefour held to the end — in beat-sized entries, because
    # the concat demuxer won't duplicate frames across one long gap
    seg += [(4, BEAT)] * 14
    assert abs(sum(d for _, d in seg) - SECONDS) < 1e-9
    return seg


def write_concat_lists():
    for aspect in ("square", "wide", "tall"):
        lines = ["ffconcat version 1.0"]
        last = None
        for state, dur in segments():
            last = f"frames/{aspect}-{state}.png"
            lines.append(f"file '{last}'")
            lines.append(f"duration {dur:.9f}")
        lines.append(f"file '{last}'")  # concat demuxer needs a final entry
        with open(os.path.join(OUT, f"list-{aspect}.txt"), "w") as f:
            f.write("\n".join(lines) + "\n")


def main():
    os.makedirs(OUT, exist_ok=True)
    write_concat_lists()
    print(f"{SECONDS}s at {BPM:.0f} BPM, synthesizing...")
    write_wav(synthesize(), os.path.join(OUT, "techno.wav"))
    print("out/techno.wav + concat lists written")


if __name__ == "__main__":
    main()
