import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { Icon } from '../components/Icon'
import { PxIcon } from '../components/PxIcon'
import {
  dungeonsInstall,
  dungeonsLaunch,
  dungeonsOpenFolder,
  dungeonsOwnership,
  dungeonsPickMods,
  dungeonsRemoveMod,
  dungeonsStatus,
  dungeonsToggleMod,
  gameOwnership,
  openUrl,
  storeGameInstall,
  storeGameOpen,
  storeGamesState,
  type DungeonsOwnership,
  type DungeonsStatus,
  type StoreGameState,
} from '../ipc/commands'
import { hasTauri } from '../ipc/tauri'
import { GAMES, gameBuyUrl, gameHero, gamePoster, gameSite, useGame, type GameInfo } from '../lib/games'
import { useAccounts } from '../state/accounts'
import { runInstall, stopInstall, useInstalls } from '../state/installs'
import { showToast, useUi } from '../state/ui'
import { BedrockServers } from '../components/playhub/BedrockServers'
import '../styles/pixel/game.css'

/*
 * Экран игры Minecraft из «Других игр» библиотеки (29.09.2026).
 * Арт на всю ширину, постер ломает рамку, под ним — название, строка,
 * факты плашками и кнопки. У Dungeons ниже — моды .pak.
 * Как запускается каждая игра — src/lib/games.ts.
 */

const JOB = 'dungeons'
// Сколько идёт по сети: манифест Mojang 12688467_cert_bugfixpatch1 (замер 29.09.2026)
// — 5,82 ГБ файлов, из них 144 есть в .lzma; с ними скачивание 2,33 ГБ.
const DUNGEONS_GB = '2,3 ГБ'

const mb = (b: number) => (b >= 1 << 20 ? (b / (1 << 20)).toFixed(1).replace('.', ',') + ' МБ' : Math.max(1, Math.round(b / 1024)) + ' КБ')

/** Имя сборки Mojang кончается датой: 12688467_cert_bugfixpatch1_2022-11-23. */
const buildDate = (v: string) => {
  const m = /(\d{4})-(\d{2})-(\d{2})$/.exec(v)
  return m ? 'Обновление ' + m[3] + '.' + m[2] + '.' + m[1] : 'Установлена'
}

const fail = (e: unknown) => showToast(String(e), 'error')

export function Game({ on }: { on: boolean }) {
  const slug = useGame((s) => s.slug)
  const game = GAMES.find((g) => g.slug === slug) || GAMES[0]
  const [store, setStore] = useState<StoreGameState[] | null>(null)
  const [tick, setTick] = useState(0)

  useEffect(() => {
    if (!on || !hasTauri()) return
    storeGamesState()
      .then(setStore)
      .catch(() => setStore([]))
  }, [on, slug, tick])

  const st = store && store.find((x) => x.slug === game.slug)
  const content = document.querySelector('.content')

  return (
    <section className={'screen' + (on ? ' on' : '')} id="s-game" key={game.slug}>
      <div className={'gm-top gm-' + game.slug}>
        <div className="gm-hero">
          <img src={gameHero(game.slug)} alt="" />
        </div>
        <img className="gm-poster" src={gamePoster(game.slug)} alt={game.name} width={180} height={270} />
        <div className="card gm-panel">
        <div className="gm-main">
          <h1>
            {game.name}
            {game.isNew ? <span className="gm-new">Новинка</span> : null}
          </h1>
          <span className="gm-tag">{game.tagline}</span>
          <div className="gm-facts">
            <span className="gm-fact">
              <PxIcon name="clock" size={18} />
              {game.released}
            </span>
            {game.slug !== 'education' ? <OwnedChip slug={game.slug} /> : null}
            {game.mods ? (
              <span className="gm-fact">
                <PxIcon name="box" size={18} />
                {game.mods}
              </span>
            ) : null}
          </div>
          {game.run === 'mojang' ? <DungeonsActions steam={!!st && st.steam} /> : <Actions game={game} st={st || null} onChanged={() => setTick((t) => t + 1)} />}
        </div>
        </div>
      </div>

      {game.run === 'mojang' ? <DungeonsMods /> : null}
      {game.slug === 'bedrock' ? <BedrockServers on={on} /> : null}

      <div className="gm-head">
        <h2>Другие игры</h2>
      </div>
      <div className="hub-grid">
        {GAMES.filter((g) => g.slug !== game.slug).map((g) => (
          <button
            key={g.slug}
            className="ph-card"
            data-sound="open"
            data-track="game_switch"
            data-id={g.slug}
            onClick={() => {
              useGame.getState().open(g.slug)
              if (content) content.scrollTop = 0
            }}
          >
            <span className="ph-card-art">
              <img src={gameHero(g.slug)} alt="" loading="lazy" />
              {g.isNew ? <span className="gm-new gm-new-card">Новинка</span> : null}
            </span>
            <span className="ph-card-body">
              <b>{g.name}</b>
              <span className="ph-card-meta">{g.meta}</span>
            </span>
          </button>
        ))}
      </div>
    </section>
  )
}

/** Владелец 29.09.2026: «нужна лицензия игры — купите в Blups». */
function LicenceNote({ slug }: { slug: GameInfo['slug'] }) {
  return (
    <button className="gm-lic" data-track="game_buy_note" data-id={slug} onClick={() => void openUrl(gameBuyUrl(slug))}>
      Нужна лицензия игры — купи в Blups
    </button>
  )
}

export function OwnedChip({ slug }: { slug: string }) {
  const own = useOwnership(slug)
  if (own !== 'owned') return null
  return (
    <span className="gm-fact gm-owned">
      <PxIcon name="check" size={18} />
      Куплена
    </span>
  )
}

/** Куплена ли игра на аккаунте Microsoft этого лаунчера (entitlements Mojang). */
function useOwnership(slug: string): DungeonsOwnership | 'load' {
  const msAccount = useAccounts((s) => s.list.find((a) => a.kind === 'microsoft') || null)
  const [own, setOwn] = useState<DungeonsOwnership | 'load'>('load')
  useEffect(() => {
    if (!msAccount) return setOwn('none')
    let alive = true
    setOwn('load')
    gameOwnership(msAccount.id, slug)
      .then((s) => alive && setOwn(s))
      .catch(() => alive && setOwn('unavailable'))
    return () => {
      alive = false
    }
  }, [msAccount, slug])
  return own
}

function openStore(slug: string, via: 'steam' | 'steam-install' | 'store-page' | 'store') {
  storeGameOpen(slug, via).catch(fail)
}

/*
 * «Играть» у игр из магазинов (владелец 29.09.2026: «нажимаешь играть —
 * скачивается игра»): есть копия — запускаем; нет — ставим, не выходя из
 * лаунчера (winget из Microsoft Store, у Education — установщик Microsoft),
 * потом запускаем. Не вышло — страница Microsoft Store. Сами файлы игр мы
 * не раздаём: Dungeons II и Legends выдаёт только магазин по покупке.
 */
export function Actions({ game, st, onChanged }: { game: GameInfo; st: StoreGameState | null; onChanged: () => void }) {
  const key = 'game-' + game.slug
  const task = useInstalls((s) => s.tasks[key])
  const openModal = useUi((s) => s.openModal)
  const own = useOwnership(game.slug)
  const page = (
    <button className="btn md ghost" data-track="game_site" onClick={() => void openUrl(gameSite(game.slug))}>
      <PxIcon name="book" size={18} /> Об игре
    </button>
  )
  if (st && !st.windows)
    return (
      <div className="gm-actions">
        <span className="gm-tag">Только Windows</span>
        {page}
      </div>
    )
  if (task && task.state === 'run')
    return (
      <div className="gm-actions">
        <div className="gm-progress">
          <div className="gm-bar">
            <span className={task.pct <= 5 ? 'gm-bar-wait' : undefined} style={{ width: Math.max(8, Math.round(task.pct)) + '%' }} />
          </div>
          <span className="gm-tag">{task.msg || task.label}</span>
          <button className="btn md ghost" onClick={() => stopInstall(key)}>
            Отменить
          </button>
        </div>
      </div>
    )
  const installed = !!st && (st.steam || st.store)
  const buy = (
    <button className="btn lg primary gm-play" data-track="game_buy" data-id={game.slug} onClick={() => void openUrl(gameBuyUrl(game.slug))}>
      <PxIcon name="gem" size={18} /> Купить в Blups
    </button>
  )
  const play = () => {
    if (st && st.steam) return openStore(game.slug, 'steam')
    if (st && st.store) return openStore(game.slug, 'store')
    runInstall({
      key,
      title: game.name,
      running: 'Ставим',
      run: () => storeGameInstall(game.slug),
      onDone: () => {
        onChanged()
        // Education запускает свой установщик сам; Store-игру — запускаем.
        if (game.slug !== 'education') setTimeout(() => openStore(game.slug, 'store'), 600)
      },
      onError: (e) => {
        showToast(String(e), 'error')
        openStore(game.slug, 'store-page')
      },
    })
  }
  // Education — лицензия школы, аккаунт Microsoft проверяет сама игра.
  const needs = game.slug !== 'education' && !installed
  if (needs && own === 'none')
    return (
      <div className="gm-actions">
        <button className="btn lg primary gm-play" data-track="game_login" data-id={game.slug} onClick={() => openModal('accModal')}>
          Войти
        </button>
        <button className="btn md secondary" data-track="game_buy" data-id={game.slug} onClick={() => void openUrl(gameBuyUrl(game.slug))}>
          Купить в Blups
        </button>
        {page}
        <LicenceNote slug={game.slug} />
      </div>
    )
  if (needs && own === 'not_owned')
    return (
      <div className="gm-actions">
        {buy}
        {/* Куплена в Store, но Mojang ещё не видит — ставим всё равно: лицензию проверит магазин. */}
        <button className="btn md secondary" data-track="game_install_anyway" data-id={game.slug} onClick={play}>
          <Icon id="i-download" /> Играть
        </button>
        {page}
        <LicenceNote slug={game.slug} />
      </div>
    )
  return (
    <div className="gm-actions">
      <button
        className="btn lg primary gm-play"
        data-sound="play"
        data-track={installed ? 'game_play' : 'game_install'}
        data-id={game.slug}
        disabled={needs && own === 'load'}
        onClick={play}
      >
        <Icon id={installed ? 'i-play' : 'i-download'} /> Играть
      </button>
      {!installed && game.steam && st && st.steamClient ? (
        <button className="btn md secondary" data-track="game_steam_install" onClick={() => openStore(game.slug, 'steam-install')}>
          Steam
        </button>
      ) : null}
      {page}
    </div>
  )
}

function DungeonsActions({ steam }: { steam: boolean }) {
  const [st, setSt] = useState<DungeonsStatus | null>(null)
  const [own, setOwn] = useState<DungeonsOwnership | 'load'>('load')
  const msAccount = useAccounts((s) => s.list.find((a) => a.kind === 'microsoft') || null)
  const task = useInstalls((s) => s.tasks[JOB])
  const openModal = useUi((s) => s.openModal)
  const done = useInstalls((s) => !!s.done[JOB])

  useEffect(() => {
    dungeonsStatus()
      .then(setSt)
      .catch(() => setSt(null))
  }, [done])

  useEffect(() => {
    if (!msAccount) return setOwn('none')
    let alive = true
    setOwn('load')
    dungeonsOwnership(msAccount.id)
      .then((s) => alive && setOwn(s))
      .catch(() => alive && setOwn('unavailable'))
    return () => {
      alive = false
    }
  }, [msAccount])

  const install = () => {
    if (!msAccount) return
    runInstall({ key: JOB, title: 'Minecraft Dungeons', running: 'Скачиваем', run: () => dungeonsInstall(msAccount.id) })
  }

  const running = task && task.state === 'run'
  const installed = !!st && st.installed
  let main: ReactNode
  if (st && !st.supported) main = <span className="gm-tag">Только Windows</span>
  else if (running)
    main = (
      <div className="gm-progress">
        <div className="gm-bar">
          <span style={{ width: Math.round(task.pct) + '%' }} />
        </div>
        <span className="gm-tag">{task.msg || task.label}</span>
        <button className="btn md ghost" onClick={() => stopInstall(JOB)}>
          Отменить
        </button>
      </div>
    )
  else if (installed)
    main = (
      <button className="btn lg primary gm-play" data-sound="play" data-track="dungeons_play" onClick={() => dungeonsLaunch().catch(fail)}>
        <Icon id="i-play" /> Играть
      </button>
    )
  else if (steam)
    main = (
      <button className="btn lg primary gm-play" data-sound="play" data-track="game_play_steam" data-id="dungeons" onClick={() => openStore('dungeons', 'steam')}>
        <Icon id="i-play" /> Играть
      </button>
    )
  else if (own === 'none')
    main = (
      <button className="btn lg primary" data-track="dungeons_login" onClick={() => openModal('accModal')}>
        Войти
      </button>
    )
  else if (own === 'not_owned')
    main = (
      <button className="btn lg primary gm-play" data-track="dungeons_buy" onClick={() => void openUrl(gameBuyUrl('dungeons'))}>
        <PxIcon name="gem" size={18} /> Купить в Blups
      </button>
    )
  else
    main = (
      <button className="btn lg primary" data-track="dungeons_install" disabled={own === 'load'} onClick={install}>
        <Icon id="i-download" /> {'Установить · ' + DUNGEONS_GB}
      </button>
    )

  return (
    <div className="gm-actions">
      {main}
      {!installed && !running && (own === 'not_owned' || own === 'none') ? <LicenceNote slug="dungeons" /> : null}
      {installed && !running ? (
        <>
          <button className="btn md secondary" data-track="dungeons_update" onClick={install} disabled={!msAccount} aria-label={buildDate(st!.version)}>
            <Icon id="i-restart" /> Обновить
          </button>
          <button className="btn md ghost" onClick={() => void dungeonsOpenFolder()}>
            <PxIcon name="folder" size={18} /> Папка
          </button>
        </>
      ) : null}
    </div>
  )
}

function DungeonsMods() {
  const [st, setSt] = useState<DungeonsStatus | null>(null)
  const done = useInstalls((s) => !!s.done[JOB])
  const reload = useCallback(() => {
    dungeonsStatus()
      .then(setSt)
      .catch(() => setSt(null))
  }, [])
  useEffect(reload, [reload, done])

  const installed = !!st && st.installed
  const mods = (st && st.mods) || []
  const addMods = () => {
    dungeonsPickMods()
      .then((n) => {
        if (n) showToast(n === 1 ? 'Мод добавлен' : 'Модов добавлено: ' + n, 'ok')
        reload()
      })
      .catch(fail)
  }
  return (
    <>
      <div className="gm-head">
        <h2>Моды</h2>
        {installed ? (
          <button className="btn md secondary" data-track="dungeons_add_mod" onClick={addMods}>
            <Icon id="i-plus" /> Добавить .pak
          </button>
        ) : null}
      </div>
      {mods.length ? (
        <div className="gm-mods">
          {mods.map((m) => (
            <div className="mod-row gm-mod" key={m.name}>
              <span className="mod-icon">
                <PxIcon name="box" size={24} />
              </span>
              <span className="mod-body">
                <span className="mod-name">
                  <b>{m.name.replace(/\.pak$/i, '')}</b>
                  <span>{mb(m.size)}</span>
                </span>
              </span>
              <button
                className={'tgl' + (m.enabled ? ' on' : '')}
                role="switch"
                aria-checked={m.enabled}
                aria-label={m.enabled ? 'Выключить' : 'Включить'}
                onClick={() => dungeonsToggleMod(m.name, !m.enabled).then(reload).catch(fail)}
              />
              <button className="btn sm ghost" aria-label="Удалить" onClick={() => dungeonsRemoveMod(m.name).then(reload).catch(fail)}>
                <Icon id="i-trash" />
              </button>
            </div>
          ))}
        </div>
      ) : (
        <div className="card gm-empty">
          <PxIcon name="chest" size={48} />
          <b>{installed ? 'Модов нет' : 'Сначала установи игру'}</b>
        </div>
      )}
    </>
  )
}
