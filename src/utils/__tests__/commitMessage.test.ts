import { describe, expect, it } from 'vitest';
import {
  COMMIT_MESSAGE_MAX_CHARS,
  commitMessageLength,
  commitMessageTooLong,
  normalizeCommitMessageLikeServer,
} from '../commitMessage';

// Mirrors `commit_message.rs`: the server strips the description and turns every
// whitespace run holding a line feed into ". " before storing it (sdk#2).
describe('upload description limit (sdk#2)', () => {
  it('turns each run of line breaks into a full stop and a space', () => {
    expect(normalizeCommitMessageLikeServer('north passage\nsump 2')).toBe('north passage. sump 2');
    expect(normalizeCommitMessageLikeServer('a  \r\n\n \t b')).toBe('a. b');
    expect(normalizeCommitMessageLikeServer('a\n\n\nb\nc')).toBe('a. b. c');
  });

  it('keeps whitespace that holds no line feed, and strips the ends like the server', () => {
    expect(normalizeCommitMessageLikeServer('a \t b')).toBe('a \t b');
    expect(normalizeCommitMessageLikeServer('a\rb')).toBe('a\rb');
    expect(normalizeCommitMessageLikeServer('\n\n  survey \n')).toBe('survey');
    expect(normalizeCommitMessageLikeServer('\u001fx\u001f')).toBe('x');
    // U+FEFF is whitespace to JS but not to the server.
    expect(normalizeCommitMessageLikeServer('﻿x')).toBe('﻿x');
  });

  it('counts characters, not UTF-16 units', () => {
    expect(commitMessageLength('café \u{1f30a}')).toBe(6);
  });

  it('charges two characters for a single line break', () => {
    const message = `${'a'.repeat(500)}\n${'b'.repeat(499)}`;
    expect(message.length).toBe(1000);
    expect(commitMessageLength(message)).toBe(1001);
    expect(commitMessageTooLong(message)).toBe(true);
  });

  it('accepts exactly the limit', () => {
    expect(commitMessageTooLong('x'.repeat(COMMIT_MESSAGE_MAX_CHARS))).toBe(false);
    expect(commitMessageTooLong(`\n  ${'x'.repeat(COMMIT_MESSAGE_MAX_CHARS)}  \n`)).toBe(false);
    expect(commitMessageTooLong('x'.repeat(COMMIT_MESSAGE_MAX_CHARS + 1))).toBe(true);
  });
});
