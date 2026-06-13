// Adaptive score, synthesised live (no audio assets, no licensing). Two beds
// crossfaded by `tension` (0 = pastoral, 1 = dystopian): a light, slightly
// daft countryside tune for when the run's going well, sinking into a Silo-ish
// industrial drone as heat and wear climb. Stings on win/loss.
//
// To swap in real composed tracks later, this is the seam: replace the synth
// graph with <audio> elements crossfaded by the same setTension().

const A4 = 440
const mtof = (m: number) => A4 * Math.pow(2, (m - 69) / 12)

// A jaunty major motif in C, 6/8-ish lilt. 0 = rest. (eighth-note grid)
const MELODY = [
  67, 0, 72, 76, 0, 72, 67, 0, 69, 0, 65, 0,
  64, 0, 67, 72, 0, 71, 67, 0, 62, 0, 67, 0,
  65, 0, 69, 72, 0, 69, 65, 0, 64, 0, 60, 0,
  62, 64, 65, 67, 0, 67, 60, 0, 0, 0, 0, 0,
]
const BASS = [36, 43, 36, 43, 41, 48, 41, 36] // oom-pah, two per bar

class Engine {
  ctx: AudioContext | null = null
  master: GainNode | null = null
  pastoral: GainNode | null = null
  dyst: GainNode | null = null
  started = false
  muted = false
  tension = 0
  timer: number | null = null
  step = 0
  nextTime = 0
  readonly spb = 0.19 // seconds per eighth-note

  /** Must be called from a user gesture (button click). Idempotent. */
  ensure() {
    if (this.started) {
      this.ctx?.resume()
      return
    }
    const AC = (window.AudioContext || (window as any).webkitAudioContext) as typeof AudioContext
    if (!AC) return
    this.ctx = new AC()
    this.master = this.ctx.createGain()
    this.master.gain.value = this.muted ? 0 : 0.22
    this.master.connect(this.ctx.destination)

    this.pastoral = this.ctx.createGain()
    this.pastoral.gain.value = 1
    this.pastoral.connect(this.master)

    this.dyst = this.ctx.createGain()
    this.dyst.gain.value = 0
    this.dyst.connect(this.master)

    this.buildDrone()
    this.started = true
    this.nextTime = this.ctx.currentTime + 0.1
    this.timer = window.setInterval(() => this.schedule(), 25)
    this.applyTension()
  }

  setMuted(m: boolean) {
    this.muted = m
    if (this.master && this.ctx) {
      this.master.gain.setTargetAtTime(m ? 0 : 0.22, this.ctx.currentTime, 0.05)
    }
  }

  setTension(t: number) {
    this.tension = Math.max(0, Math.min(1, t))
    this.applyTension()
  }

  private applyTension() {
    if (!this.ctx || !this.pastoral || !this.dyst) return
    // Bias the crossfade so it reads as "switching" near the middle.
    const d = Math.pow(this.tension, 1.4)
    const now = this.ctx.currentTime
    this.pastoral.gain.setTargetAtTime((1 - d) * 0.9, now, 0.8)
    this.dyst.gain.setTargetAtTime(d, now, 0.8)
  }

  // --- the dystopian bed: low detuned drone + slow pulse + faint high unease
  private buildDrone() {
    if (!this.ctx || !this.dyst) return
    const c = this.ctx
    const mk = (freq: number, type: OscillatorType, g: number) => {
      const o = c.createOscillator()
      o.type = type
      o.frequency.value = freq
      const gain = c.createGain()
      gain.gain.value = g
      o.connect(gain)
      gain.connect(this.dyst!)
      o.start()
      return { o, gain }
    }
    mk(mtof(33), 'sawtooth', 0.16) // A1 drone
    mk(mtof(33) * 1.003, 'sawtooth', 0.14) // detuned beat
    mk(mtof(44), 'sine', 0.1) // G#2-ish dissonance
    mk(mtof(68), 'sine', 0.025) // faint high drone
    // slow tremolo on the whole bed
    const lfo = c.createOscillator()
    lfo.frequency.value = 0.12
    const lfoGain = c.createGain()
    lfoGain.gain.value = 0.5
    lfo.connect(lfoGain)
    lfoGain.connect(this.dyst.gain)
    lfo.start()
  }

  // --- scheduler: pastoral melody + bass, plus occasional dystopian clanks
  private schedule() {
    if (!this.ctx) return
    while (this.nextTime < this.ctx.currentTime + 0.1) {
      const i = this.step % MELODY.length
      const note = MELODY[i]
      if (note > 0) this.pluck(mtof(note), this.nextTime, 0.16, 'triangle', 0.12)
      if (i % 6 === 0) {
        const b = BASS[(this.step / 6) % BASS.length | 0]
        this.pluck(mtof(b), this.nextTime, 0.22, 'triangle', 0.16)
      }
      // metallic clank when things are bad
      if (this.tension > 0.45 && Math.random() < 0.04 * this.tension) {
        this.clank(this.nextTime)
      }
      this.nextTime += this.spb
      this.step++
    }
  }

  private pluck(freq: number, t: number, dur: number, type: OscillatorType, vol: number) {
    if (!this.ctx || !this.pastoral) return
    const o = this.ctx.createOscillator()
    o.type = type
    o.frequency.value = freq
    const g = this.ctx.createGain()
    g.gain.setValueAtTime(0, t)
    g.gain.linearRampToValueAtTime(vol, t + 0.01)
    g.gain.exponentialRampToValueAtTime(0.0008, t + dur)
    o.connect(g)
    g.connect(this.pastoral)
    o.start(t)
    o.stop(t + dur + 0.02)
  }

  private clank(t: number) {
    if (!this.ctx || !this.dyst) return
    const c = this.ctx
    const buf = c.createBuffer(1, c.sampleRate * 0.3, c.sampleRate)
    const data = buf.getChannelData(0)
    for (let i = 0; i < data.length; i++) data[i] = (Math.random() * 2 - 1) * Math.pow(1 - i / data.length, 3)
    const src = c.createBufferSource()
    src.buffer = buf
    const bp = c.createBiquadFilter()
    bp.type = 'bandpass'
    bp.frequency.value = 1800 + Math.random() * 2200
    bp.Q.value = 8
    const g = c.createGain()
    g.gain.value = 0.12
    src.connect(bp)
    bp.connect(g)
    g.connect(this.dyst)
    src.start(t)
  }

  /** One-shot cue on a decisive outcome. */
  sting(kind: 'win' | 'loss') {
    if (!this.ctx || !this.master) return
    const t = this.ctx.currentTime + 0.02
    if (kind === 'win') {
      // bright, slightly comic major arpeggio
      ;[60, 64, 67, 72, 76].forEach((n, i) =>
        this.pluck(mtof(n), t + i * 0.09, 0.3, 'triangle', 0.2),
      )
    } else {
      // low thud + descending dissonant cluster
      this.pluck(mtof(31), t, 0.8, 'sawtooth', 0.25)
      ;[58, 56, 53, 49].forEach((n, i) =>
        this.pluck(mtof(n), t + 0.12 + i * 0.13, 0.5, 'sawtooth', 0.12),
      )
    }
  }
}

export const audio = new Engine()

/** Map game state to a 0..1 tension for the crossfade. */
export function tensionFromState(heat: number, wear: number, outcome: string): number {
  if (outcome === 'lost') return 1
  if (outcome === 'won') return 0
  return Math.min(1, (heat / 10) * 0.6 + (wear / 10) * 0.45)
}
