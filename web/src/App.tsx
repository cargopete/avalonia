import { createSignal, onCleanup, onMount, For, Show } from 'solid-js'
import { Backdrop, Portrait } from './art'

// Thin client (RFC-AVL-001 R1): render state, post intents. No game rules here.

type Choice = { label: string; enabled: boolean; note: string | null }
type Scene = { title: string; body: string[]; choices: Choice[]; speaker: string | null }
type Mission = {
  terrain: string
  terrain_name: string
  cargo: string
  qty_desc: string
  dest_name: string
  illicit: boolean
  reward_pence: number
  cash_pence: number
  fuel_l: number
  fuel_pct: number
  max_fuel_l: number
  wear: number
  heat: number
  leg: number
  legs_total: number
  fixer: string
  antagonist: string
  outcome: 'in_progress' | 'won' | 'lost'
  reason: string | null
  haul_pence: number | null
  scene: Scene
}
type Scores = { played: number; won: number; lost: number; best_haul_pence: number }
type View = { active: boolean; mission: Mission | null; scores: Scores }

const NPC_NAMES: Record<string, string> = {
  arthur: 'Arthur Pidgeon',
  finch: 'Mr. Finch',
  hobbs: 'Corporal Hobbs',
  carver: 'Sal Carver',
  tansy: 'Mrs. Tansy',
  wray: 'Mr. Wray',
}
const NPC_ROLES: Record<string, string> = {
  arthur: 'the fixer',
  finch: 'Joint Committee',
  hobbs: 'Collective Force',
  carver: 'Carver Haulage',
  tansy: 'The Pelican',
  wray: 'seed merchant',
}

function money(pence: number): string {
  const sign = pence < 0 ? '-' : ''
  const p = Math.abs(pence)
  const l = Math.floor(p / 240)
  const s = Math.floor((p % 240) / 12)
  const d = p % 12
  if (l > 0) return `${sign}£${l} ${s}s`
  if (s > 0) return d > 0 ? `${sign}${s}s ${d}d` : `${sign}${s}s`
  return `${sign}${d}d`
}

export default function App() {
  const [view, setView] = createSignal<View | null>(null)
  const [chat, setChat] = createSignal<string[]>([])
  const [chatBusy, setChatBusy] = createSignal(false)
  const [llm, setLlm] = createSignal<boolean | null>(null)
  let busy = false
  let chatInput: HTMLInputElement | undefined

  const m = () => view()?.mission ?? null

  const refresh = async () => setView(await (await fetch('/api/view')).json())

  const newRun = async () => {
    setChat([])
    setView(await (await fetch('/api/new', { method: 'POST' })).json())
  }
  const abandon = async () => {
    await fetch('/api/abandon', { method: 'POST' })
    setChat([])
    refresh()
  }
  const choose = async (idx: number) => {
    const mm = m()
    if (busy || !mm || !mm.scene.choices[idx]?.enabled || mm.outcome !== 'in_progress') return
    busy = true
    try {
      setView(await (await fetch('/api/choose', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ idx }),
      })).json())
    } finally {
      busy = false
    }
  }
  const sendChat = async (e: Event) => {
    e.preventDefault()
    const text = chatInput?.value.trim()
    if (!text || chatBusy()) return
    chatInput!.value = ''
    setChat((c) => [...c, `You — ${text}`])
    setChatBusy(true)
    try {
      const r = await fetch('/api/say', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ npc: '', text }),
      })
      const d = await r.json()
      const who = NPC_NAMES[d.npc] ?? 'The fixer'
      setChat((c) => [...c, `${who} — ${d.line}`])
    } catch {
      setChat((c) => [...c, 'The fixer — (the depot telephone is down)'])
    } finally {
      setChatBusy(false)
    }
  }

  onMount(() => {
    refresh()
    fetch('/api/status').then((r) => r.json()).then((s) => setLlm(!!s.llm)).catch(() => setLlm(false))
    const onKey = (e: KeyboardEvent) => {
      if (document.activeElement === chatInput) return
      const mm = m()
      if (!mm || mm.outcome !== 'in_progress') return
      if (e.key >= '1' && e.key <= '9') {
        e.preventDefault()
        choose(Number(e.key) - 1)
      } else if (e.code === 'Space') {
        const en = mm.scene.choices.filter((c) => c.enabled)
        if (en.length === 1) {
          e.preventDefault()
          choose(mm.scene.choices.findIndex((c) => c.enabled))
        }
      }
    }
    window.addEventListener('keydown', onKey)
    // Tab close / navigate away ends the run (DR: ephemeral session).
    const bail = () => { try { fetch('/api/abandon', { method: 'POST', keepalive: true }) } catch {} }
    window.addEventListener('pagehide', bail)
    onCleanup(() => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('pagehide', bail)
    })
  })

  return (
    <Show when={view()} fallback={<div class="boot">Reaching the depot…</div>}>
      {(v) => (
        <Show when={v().active && m()} fallback={<StartScreen scores={v().scores} llm={llm()} onStart={newRun} />}>
          <GameScreen
            m={m()!}
            chat={chat()}
            chatBusy={chatBusy()}
            sendChat={sendChat}
            setChatRef={(el) => (chatInput = el)}
            choose={choose}
            newRun={newRun}
            abandon={abandon}
          />
        </Show>
      )}
    </Show>
  )
}

function StartScreen(props: { scores: Scores; llm: boolean | null; onStart: () => void }) {
  return (
    <div class="start">
      <div class="start-card">
        <h1>AVALON</h1>
        <p class="tag">One truck. One run. The roads keep no friends.</p>
        <button class="primary" onClick={props.onStart}>Begin a run</button>
        <div class="scores">
          <span>{props.scores.played} run{props.scores.played === 1 ? '' : 's'}</span>
          <span class="won">{props.scores.won} delivered</span>
          <span class="lost">{props.scores.lost} lost</span>
          <Show when={props.scores.best_haul_pence > 0}>
            <span>best haul {money(props.scores.best_haul_pence)}</span>
          </Show>
        </div>
        <p class="oracle">
          {props.llm === null ? '' : props.llm ? 'the fixer is in' : 'the fixer is out — canned lines only'}
        </p>
      </div>
    </div>
  )
}

function Gauge(props: { label: string; pct: number; tone: 'fuel' | 'wear' | 'heat'; text: string }) {
  return (
    <div class={`gauge ${props.tone}`} classList={{ danger: props.pct >= 80 && props.tone !== 'fuel', low: props.pct <= 25 && props.tone === 'fuel' }}>
      <div class="gauge-label">{props.label}</div>
      <div class="gauge-bar"><div class="gauge-fill" style={{ width: `${Math.max(0, Math.min(100, props.pct))}%` }} /></div>
      <div class="gauge-text">{props.text}</div>
    </div>
  )
}

function GameScreen(props: {
  m: Mission
  chat: string[]
  chatBusy: boolean
  sendChat: (e: Event) => void
  setChatRef: (el: HTMLInputElement) => void
  choose: (i: number) => void
  newRun: () => void
  abandon: () => void
}) {
  const m = () => props.m
  const over = () => m().outcome !== 'in_progress'
  const briefing = () => m().scene.title === 'The Job'
  const speaker = () => m().scene.speaker

  return (
    <main class="game">
      <header class="topbar">
        <div class="run-id">
          <strong>{m().qty_desc} of {m().cargo}</strong>
          <span> → {m().dest_name} · {m().terrain_name}</span>
          <Show when={m().illicit}><span class="illicit"> · unlicensed</span></Show>
        </div>
        <div class="reward">haul {money(m().reward_pence)}</div>
      </header>

      <RouteMap leg={m().leg} total={m().legs_total} dest={m().dest_name} />

      <section class="hud">
        <Gauge label="Fuel" tone="fuel" pct={m().fuel_pct} text={`${m().fuel_l}/${m().max_fuel_l} L`} />
        <Gauge label="Condition" tone="wear" pct={m().wear * 10} text={`${10 - m().wear}/10`} />
        <Gauge label="Heat" tone="heat" pct={m().heat * 10} text={`${m().heat}/10`} />
        <div class="purse">purse {money(m().cash_pence)}</div>
      </section>

      <section class="stage">
        <Backdrop terrain={m().terrain} />
        <div class="stage-overlay">
          <Show when={speaker()}>
            <Portrait
              id={speaker()!}
              name={NPC_NAMES[speaker()!] ?? speaker()!}
              role={NPC_ROLES[speaker()!]}
              speaking={!over()}
            />
          </Show>
          <div class="panel" classList={{ won: m().outcome === 'won', lost: m().outcome === 'lost' }}>
            <h2>{m().scene.title}</h2>
            <For each={m().scene.body}>{(p) => <p>{p}</p>}</For>
          </div>
        </div>
      </section>

      <Show
        when={!over()}
        fallback={
          <section class="verdict">
            <button class="primary" onClick={props.newRun}>Begin a new run</button>
          </section>
        }
      >
        <section class="choices">
          <For each={m().scene.choices}>
            {(c, i) => (
              <button class="choice" disabled={!c.enabled} onClick={() => props.choose(i())}>
                <span class="key">{i() + 1}</span>
                <span class="choice-label">{c.label}</span>
                <Show when={c.note}><span class="choice-note">{c.note}</span></Show>
              </button>
            )}
          </For>
        </section>
      </Show>

      <Show when={briefing() && !over()}>
        <section class="chat">
          <For each={props.chat}>{(line) => <p>{line}</p>}</For>
          <form onSubmit={props.sendChat}>
            <input
              ref={props.setChatRef}
              placeholder={props.chatBusy ? 'The fixer considers…' : 'Ask the fixer something…'}
              disabled={props.chatBusy}
              maxlength="200"
            />
          </form>
        </section>
      </Show>

      <Show when={!over()}>
        <button class="abandon" onClick={props.abandon}>abandon run</button>
      </Show>
    </main>
  )
}

function RouteMap(props: { leg: number; total: number; dest: string }) {
  const nodes = () => Array.from({ length: props.total + 1 }, (_, i) => i)
  return (
    <div class="routemap">
      <span class="route-end">Wychford</span>
      <div class="route-line">
        <For each={nodes()}>
          {(i) => (
            <div
              class="route-node"
              classList={{ done: i <= props.leg, here: i === props.leg && props.leg > 0 }}
            />
          )}
        </For>
      </div>
      <span class="route-end">{props.dest}</span>
    </div>
  )
}
