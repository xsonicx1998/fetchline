import { useState } from 'react'
import type { DownloadRow } from './types'
import type { SidebarFilter } from '../components/CategorySidebar'

/** Round 75 — the sidebar selection state, extracted from App.tsx
 *  into its own module so the sidebar test renders the REAL
 *  selection functions the app uses, not a copy of them.
 *
 *  One list at a time: picking a category, a queue or a filter
 *  clears the other two. The sidebar must therefore call exactly
 *  ONE handler per click — calling all three (the pre-Round-75
 *  bug) let React batch the setStates with the last write winning,
 *  so a category click ended with selectedCategory null. */
export function useSidebarSelection() {
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null)
  const [selectedQueue, setSelectedQueue] = useState<string | null>(null)
  const [selectedFilter, setSelectedFilter] = useState<SidebarFilter>('all')
  const [selectedGrabberId, setSelectedGrabberId] = useState<string | null>(null)

  function selectCategory(id: string | null) {
    setSelectedCategory(id)
    setSelectedQueue(null)
    setSelectedFilter('all')
    setSelectedGrabberId(null)
  }

  function selectQueue(id: string | null) {
    setSelectedQueue(id)
    setSelectedCategory(null)
    setSelectedFilter('all')
    setSelectedGrabberId(null)
  }

  function selectFilter(filter: SidebarFilter, grabberId: string | null = null) {
    setSelectedFilter(filter)
    setSelectedGrabberId(grabberId)
    setSelectedCategory(null)
    setSelectedQueue(null)
  }

  return {
    selectedCategory,
    selectedQueue,
    selectedFilter,
    selectedGrabberId,
    selectCategory,
    selectQueue,
    selectFilter,
  }
}

export type SidebarSelection = ReturnType<typeof useSidebarSelection>

/** Round 76 (6) — Delete Completed: exactly the rows whose
 *  status is 'complete' — nothing else. The scope is the WHOLE
 *  list, not just the visible view: a finished download in
 *  another category or queue is completed too, and deleting it
 *  is what the user saw (and accepted). The engine only
 *  soft-deletes the given row, so this selection IS the whole
 *  behaviour — pin it with a test, do not add a confirmation. */
export function selectCompletedDownloads(downloads: DownloadRow[]): DownloadRow[] {
  return downloads.filter((d) => d.status === 'complete')
}

/** Which rows the main table shows for a selection — the same
 *  computation App.tsx used inline. A selected queue shows the
 *  queue's files (loaded separately by the app); otherwise the
 *  status filter, then the grabber project, then the category,
 *  then everything. The search query narrows whatever list won. */
export function selectVisibleRows(args: {
  downloads: DownloadRow[]
  queueFiles: DownloadRow[]
  selectedCategory: string | null
  selectedQueue: string | null
  selectedFilter: SidebarFilter
  selectedGrabberId: string | null
  searchQuery: string
}): DownloadRow[] {
  const {
    downloads,
    queueFiles,
    selectedCategory,
    selectedQueue,
    selectedFilter,
    selectedGrabberId,
    searchQuery,
  } = args
  let rows: DownloadRow[]
  if (selectedQueue !== null) rows = queueFiles
  else if (selectedFilter === 'unfinished') rows = downloads.filter((d) => d.status !== 'complete')
  else if (selectedFilter === 'finished') rows = downloads.filter((d) => d.status === 'complete')
  else if (selectedFilter === 'grabber')
    rows = downloads.filter((d) => d.grabber_project_id === selectedGrabberId)
  else if (selectedCategory === null) rows = downloads
  else rows = downloads.filter((d) => d.category_id === selectedCategory)
  const query = searchQuery.trim().toLowerCase()
  if (query) rows = rows.filter((d) => d.filename.toLowerCase().includes(query))
  return rows
}
