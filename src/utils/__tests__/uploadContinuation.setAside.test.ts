/**
 * The description "Resolve now" sets aside: kept for the next upload dialog with
 * no upload pending, taken over by the next continuation written, and gone once
 * it no longer describes anything (nothing left to upload, another project, another
 * account).
 */
import { beforeEach, expect, it } from 'vitest';
import {
  captureUploadOwner, getSetAsideUploadMessage, getUploadContinuation, setAsideUploadMessage, setUploadContinuation,
} from '../uploadContinuation';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';

const PATH = '/synthetic/aside.stalx';
const LINK = { editState: 'editing', hasUnpushedEdits: true } as SpeleoLinkState;
let lifetime = 0;

beforeEach(() => {
  // A fresh project lifetime clears both slots, whatever an earlier test left.
  lifetime += 1;
  useProjectStore.setState({ project: { file_path: PATH, created_at: `t${lifetime}` } as never, isDirty: false });
  useSpeleoStore.setState({
    link: { ...LINK },
    session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' },
  });
});

it('holds the message with no upload pending', () => {
  setUploadContinuation({ path: PATH, message: 'pending', closeAfterSave: true });
  setAsideUploadMessage('Fixed the sump', captureUploadOwner());
  expect(getUploadContinuation(PATH)).toBeNull();
  expect(getSetAsideUploadMessage(PATH)).toBe('Fixed the sump');
});

it('survives another dialog dropping its own pending upload', () => {
  setAsideUploadMessage('Fixed the sump', captureUploadOwner());
  setUploadContinuation(null);
  expect(getSetAsideUploadMessage(PATH)).toBe('Fixed the sump');
});

it('is taken over by the next pending upload written', () => {
  setAsideUploadMessage('Fixed the sump', captureUploadOwner());
  setUploadContinuation({ path: PATH, message: 'Fixed the sump', closeAfterSave: false });
  expect(getSetAsideUploadMessage(PATH)).toBeNull();
});

it('lapses once nothing is left to upload', () => {
  setAsideUploadMessage('Fixed the sump', captureUploadOwner());
  useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: false } });
  expect(getSetAsideUploadMessage(PATH)).toBeNull();
  // It does not come back with new edits: it described the uploaded ones.
  useSpeleoStore.setState({ link: { ...LINK } });
  expect(getSetAsideUploadMessage(PATH)).toBeNull();
});

it('keeps an empty message as nothing', () => {
  setAsideUploadMessage('   ', captureUploadOwner());
  expect(getSetAsideUploadMessage(PATH)).toBeNull();
});

it.each(['project', 'account'])('belongs to its %s only', (kind) => {
  setAsideUploadMessage('Fixed the sump', captureUploadOwner());
  if (kind === 'project') useProjectStore.setState({ project: { file_path: '/synthetic/other.stalx' } as never });
  else useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'other@example.com' } });
  expect(getSetAsideUploadMessage(PATH)).toBeNull();
  expect(getSetAsideUploadMessage('/synthetic/other.stalx')).toBeNull();
});
