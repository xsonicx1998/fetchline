import type { CategoryRow } from './types'

/** Round 87.5 — the file-extension → category mapping, shared
 *  by the Add URL dialog's auto-pick. It mirrors the engine's
 *  `category_for_extension_filtered` (store.rs): the first
 *  category whose extension list holds the file's extension
 *  (case-insensitive), honouring the `file_types_auto`
 *  allow-list — a non-empty list restricts auto-assign to the
 *  listed extensions and anything else falls back to Documents
 *  — and an unknown extension falls back to Documents too.
 *
 *  Two dialog-side differences from the engine, both
 *  deliberate: a file with NO extension returns null (the
 *  dialog leaves the category untouched instead of jumping to
 *  Documents while the user is still typing a name), and a
 *  computed category that is not in the passed list (e.g.
 *  Documents was deleted) returns null too — the engine still
 *  auto-assigns on submit when the dialog sends no category,
 *  so nothing is lost by leaving the select alone. */

/** The extension of a filename (lowercase, no dot): the part
 *  after the last `.`. Empty when there is none — a trailing
 *  dot, a dotfile (`.gitignore`) and a bare name all count as
 *  "no extension yet". */
export function extensionOfFilename(filename: string): string {
  const name = filename.trim()
  const idx = name.lastIndexOf('.')
  if (idx <= 0) return ''
  return name.slice(idx + 1).toLowerCase()
}

export function categoryForExtension(
  categories: CategoryRow[],
  filename: string,
  autoFileTypes: string[] = [],
): string | null {
  const ext = extensionOfFilename(filename)
  if (!ext) return null
  const auto = autoFileTypes.map((e) => e.trim().replace(/^\./, '').toLowerCase())
  let id: string
  if (auto.length > 0 && !auto.includes(ext)) {
    id = 'cat-documents'
  } else {
    const match = categories.find((c) =>
      c.file_extensions.some((e) => e.toLowerCase() === ext),
    )
    id = match ? match.id : 'cat-documents'
  }
  return categories.some((c) => c.id === id) ? id : null
}
