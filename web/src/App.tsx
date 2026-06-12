import { createSignal, onCleanup, onMount, For, Show } from 'solid-js'

// Thin client (RFC-AVL-001 R1): render state, post intents. No game rules here.

type StateView = {
  tick: number
  day: number
  phase: string
  cash_pence: number
  fuel_l: number
  diesel_price_pence: number
  bedford_wear: number
  guild_suspicion: number
  runs_completed: number
  cargo: string | null
}

type Choice = { label: string; enabled: boolean; note: string | null }
type Scene = { title: string; body: string[]; choices: Choice[] }
type View = { state: StateView; scene: Scene }
type SimEvent = { kind: string } & Record<string, unknown>

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
  const [log, setLog] = createSignal<string[]>([])
  let busy = false

  const refresh = async () => {
    const res = await fetch('/api/view')
    setView(await res.json())
  }

  const choose = async (idx: number) => {
    if (busy) return
    const v = view()
    if (!v || !v.scene.choices[idx]?.enabled) return
    busy = true
    try {
      const res = await fetch('/api/choose', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ idx }),
      })
      setView(await res.json())
    } finally {
      busy = false
    }
  }

  const describe = (ev: SimEvent): string | null => {
    switch (ev.kind) {
      case 'narration':
        return ev.text as string
      case 'price_drift':
        return `Diesel in ${ev.settlement}: ${money(ev.price_pence as number)}/L.`
      default:
        return null
    }
  }

  onMount(() => {
    refresh()
    const es = new EventSource('/api/stream')
    es.onmessage = (msg) => {
      const line = describe(JSON.parse(msg.data))
      if (line) setLog((l) => [line, ...l].slice(0, 80))
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key >= '1' && e.key <= '9') {
        e.preventDefault()
        choose(Number(e.key) - 1)
      } else if (e.code === 'Space' || e.code === 'Enter') {
        const v = view()
        if (!v) return
        const enabled = v.scene.choices.filter((c) => c.enabled)
        if (enabled.length === 1) {
          e.preventDefault()
          choose(v.scene.choices.findIndex((c) => c.enabled))
        }
      }
    }
    window.addEventListener('keydown', onKey)
    onCleanup(() => {
      es.close()
      window.removeEventListener('keydown', onKey)
    })
  })

  return (
    <main>
      <header>
        <h1>AVALON</h1>
        <p class="sub">Form 4B not required &mdash; yet.</p>
      </header>
      <Show when={view()} fallback={<p>Reaching the depot&hellip;</p>}>
        {(v) => (
          <>
            <section class="ledger">
              <div class="stat">
                <span class="label">Day</span>
                <span class="value">{v().state.day}</span>
              </div>
              <div class="stat">
                <span class="label">Phase</span>
                <span class="value">{v().state.phase}</span>
              </div>
              <div class="stat">
                <span class="label">Cash</span>
                <span class="value">{money(v().state.cash_pence)}</span>
              </div>
              <div class="stat">
                <span class="label">Fuel</span>
                <span class="value">{v().state.fuel_l} L</span>
              </div>
              <div class="stat">
                <span class="label">Wear</span>
                <span class="value">{v().state.bedford_wear}/10</span>
              </div>
              <div class="stat">
                <span class="label">Suspicion</span>
                <span class="value">{v().state.guild_suspicion}/10</span>
              </div>
            </section>
            <Show when={v().state.cargo}>
              <p class="cargo">Under the sheeting: {v().state.cargo}</p>
            </Show>
            <section class="scene">
              <h2>{v().scene.title}</h2>
              <For each={v().scene.body}>{(para) => <p>{para}</p>}</For>
              <ol class="choices">
                <For each={v().scene.choices}>
                  {(c, i) => (
                    <li>
                      <button
                        class={c.enabled ? '' : 'disabled'}
                        disabled={!c.enabled}
                        onClick={() => choose(i())}
                      >
                        <span class="key">{i() + 1}</span> {c.label}
                        <Show when={c.note}>
                          <span class="note"> — {c.note}</span>
                        </Show>
                      </button>
                    </li>
                  )}
                </For>
              </ol>
            </section>
            <p class="hint">
              <kbd>1</kbd>&ndash;<kbd>{v().scene.choices.length}</kbd> to choose
              <Show when={v().scene.choices.filter((c) => c.enabled).length === 1}>
                {' '}&middot; <kbd>Space</kbd> to continue
              </Show>
            </p>
          </>
        )}
      </Show>
      <section class="log">
        <For each={log()}>{(line) => <p>{line}</p>}</For>
      </section>
    </main>
  )
}
