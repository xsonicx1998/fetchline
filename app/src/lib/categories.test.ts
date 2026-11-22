// Round 87.5 (E4) — the file-extension → category mapping the
// Add URL dialog auto-picks from. It mirrors the engine's
// `category_for_extension_filtered` (store.rs): first category
// whose extension list holds the file's extension, the
// `file_types_auto` allow-list restricting auto-assign, and
// the Documents fallback for unknown extensions.
//
// Run with: npm run test:ui

import { describe, expect, test } from 'vitest'
import { categoryForExtension, extensionOfFilename } from './categories'
import type { CategoryRow } from './types'

function cat(id: string, name: string, extensions: string[]): CategoryRow {
  return { id, name, default_folder: '', file_extensions: extensions, is_builtin: true }
}

const BUILTINS = [
  cat('cat-music', 'Music', ['mp3', 'flac', 'wav', 'aac', 'ogg']),
  cat('cat-video', 'Video', ['mp4', 'mkv', 'avi', 'mov', 'webm']),
  cat('cat-documents', 'Documents', ['pdf', 'doc', 'docx', 'txt', 'epub']),
  cat('cat-programs', 'Programs', ['exe', 'msi']),
  cat('cat-compressed', 'Compressed', ['zip', 'rar', '7z', 'tar', 'gz']),
]

describe('extensionOfFilename', () => {
  test('takes the part after the last dot, lowercased', () => {
    expect(extensionOfFilename('movie.MP4')).toBe('mp4')
    expect(extensionOfFilename('archive.tar.gz')).toBe('gz')
    expect(extensionOfFilename('song.mp3')).toBe('mp3')
  })

  test('no extension: bare name, trailing dot, dotfile', () => {
    expect(extensionOfFilename('download')).toBe('')
    expect(extensionOfFilename('report.')).toBe('')
    expect(extensionOfFilename('.gitignore')).toBe('')
    expect(extensionOfFilename('')).toBe('')
  })
})

describe('categoryForExtension', () => {
  test('maps each builtin extension to its category', () => {
    expect(categoryForExtension(BUILTINS, 'song.mp3')).toBe('cat-music')
    expect(categoryForExtension(BUILTINS, 'movie.mp4')).toBe('cat-video')
    expect(categoryForExtension(BUILTINS, 'setup.exe')).toBe('cat-programs')
    expect(categoryForExtension(BUILTINS, 'backup.zip')).toBe('cat-compressed')
    expect(categoryForExtension(BUILTINS, 'notes.pdf')).toBe('cat-documents')
  })

  test('matching is case-insensitive', () => {
    expect(categoryForExtension(BUILTINS, 'SONG.MP3')).toBe('cat-music')
    expect(categoryForExtension(BUILTINS, 'Movie.MKV')).toBe('cat-video')
  })

  test('an unknown extension falls back to Documents, like the engine', () => {
    expect(categoryForExtension(BUILTINS, 'data.xyz')).toBe('cat-documents')
  })

  test('a file with no extension leaves the category alone (null)', () => {
    expect(categoryForExtension(BUILTINS, 'download')).toBeNull()
    expect(categoryForExtension(BUILTINS, '')).toBeNull()
  })

  test('the first matching category wins (custom categories first)', () => {
    const custom = cat('cat-custom', 'Clips', ['mp4', 'mov'])
    expect(categoryForExtension([custom, ...BUILTINS], 'clip.mp4')).toBe('cat-custom')
  })

  test('a computed category that is not in the list returns null', () => {
    const withoutDocuments = BUILTINS.filter((c) => c.id !== 'cat-documents')
    expect(categoryForExtension(withoutDocuments, 'data.xyz')).toBeNull()
  })

  test('file_types_auto restricts auto-assign to the listed types', () => {
    // mp3 is not in the allow-list: Documents, like the engine.
    expect(categoryForExtension(BUILTINS, 'song.mp3', ['mp4'])).toBe('cat-documents')
    // mp4 is listed: the normal mapping applies.
    expect(categoryForExtension(BUILTINS, 'movie.mp4', ['mp4'])).toBe('cat-video')
    // An empty list means every type auto-assigns.
    expect(categoryForExtension(BUILTINS, 'song.mp3', [])).toBe('cat-music')
  })
})
