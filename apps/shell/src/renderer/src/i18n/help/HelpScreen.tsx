import { useMemo, useState } from 'react'
import { Markdown } from '@genoffice/ui'
import { HELP_GROUPS, HELP_TOPICS, helpBody, searchTopics } from './help-registry'

/**
 * The in-app manual (issue #1520): sidebar topic list with full-text search
 * over a markdown body per topic. Rendered by the shell renderer when the
 * view is loaded at ?mode=help (the Help tab / F1 / Help menu).
 *
 * Topics ship in zh + en; every other locale falls back to en (the registry
 * makes the gap visible rather than hiding it).
 */
export function HelpScreen(): React.ReactElement {
  const zh = document.documentElement.lang.startsWith('zh')
  const [query, setQuery] = useState('')
  const [activeId, setActiveId] = useState(HELP_TOPICS[0]!.id)
  const hits = useMemo(() => searchTopics(query, zh ? 'zh' : 'en'), [query, zh])

  const active = HELP_TOPICS.find((t) => t.id === activeId) ?? null
  const body = active ? helpBody(active.id, zh ? 'zh' : 'en') : null
  const totalHits = HELP_TOPICS.filter((t) => hits.has(t.id)).length

  return (
    <div className="help-root">
      <aside className="help-side">
        <div className="help-search-row">
          <input
            className="help-search"
            type="search"
            placeholder={zh ? '搜索手册…' : 'Search the manual…'}
            value={query}
            aria-label={zh ? '搜索手册' : 'Search the manual'}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query && (
            <span className="help-hit-count">
              {zh ? `${totalHits} 个主题` : `${totalHits} ${totalHits === 1 ? 'topic' : 'topics'}`}
            </span>
          )}
        </div>
        <nav className="help-nav" aria-label={zh ? '手册目录' : 'Manual contents'}>
          {HELP_GROUPS.map((g) => {
            const topics = HELP_TOPICS.filter((t) => t.group === g.id && hits.has(t.id))
            if (topics.length === 0) return null
            return (
              <div key={g.id} className="help-group">
                <div className="help-group-title">{zh ? g.titleZh : g.titleEn}</div>
                {topics
                  .slice()
                  .sort((a, b) => a.order - b.order)
                  .map((t) => (
                    <button
                      key={t.id}
                      type="button"
                      className={`help-topic${t.id === activeId ? ' active' : ''}`}
                      onClick={() => setActiveId(t.id)}
                    >
                      {zh ? t.titleZh : t.titleEn}
                    </button>
                  ))}
              </div>
            )
          })}
        </nav>
      </aside>
      <main className="help-main">
        {active ? (
          <article className="help-article" key={active.id}>
            <h1>{zh ? active.titleZh : active.titleEn}</h1>
            {body !== null ? (
              <Markdown text={body} />
            ) : (
              <p className="help-missing">
                {zh ? '本主题的内容尚未编写。' : 'This topic has not been written yet.'}
              </p>
            )}
          </article>
        ) : null}
      </main>
    </div>
  )
}
