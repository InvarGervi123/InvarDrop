// @vitest-environment jsdom
import {afterEach,beforeEach,it,expect,vi} from 'vitest';
import {cleanup,render,screen,fireEvent,waitFor} from '@testing-library/react';
import {invoke} from '@tauri-apps/api/core';
import {ProcessingPanel} from './ProcessingPanel';
vi.mock('@tauri-apps/api/core',()=>({invoke:vi.fn(),isTauri:()=>true}));
const call=vi.mocked(invoke);
beforeEach(()=>{call.mockImplementation(async command=>command==='processing_snapshot'?[{id:'job',name:'song.ogg',state:'choice',done:0,total:1,error:null,path:null}]:undefined);});
afterEach(()=>{cleanup();vi.resetAllMocks();});
it('keeps originals as default and never converts on detection',async()=>{render(<ProcessingPanel/>);await screen.findByText('song.ogg');expect(call.mock.calls.every(([c])=>c==='processing_snapshot')).toBe(true);fireEvent.click(screen.getByText('Keep originals'));await waitFor(()=>expect(call).toHaveBeenCalledWith('processing_decide',{id:'job',convert:false}));});
it('converts only after explicit action and blocks duplicate pending decisions',async()=>{call.mockImplementation(async command=>command==='processing_snapshot'?[{id:'job',name:'song.ogg',state:'choice',done:0,total:1,error:null,path:null}]:new Promise(()=>{}));render(<ProcessingPanel/>);await screen.findByText('song.ogg');fireEvent.click(screen.getByText('Convert OGG to MP3'));fireEvent.click(screen.getByText('Convert OGG to MP3'));expect(call.mock.calls.filter(([c])=>c==='processing_decide')).toEqual([['processing_decide',{id:'job',convert:true}]]);});
it('shows processing failure and sends cancellation by id only',async()=>{render(<ProcessingPanel/>);await screen.findByText('song.ogg');fireEvent.click(screen.getByText('Cancel processing'));await waitFor(()=>expect(call).toHaveBeenCalledWith('processing_cancel',{id:'job'}));});
it('shows empty completed extraction as finished rather than running',async()=>{call.mockImplementation(async command=>command==='processing_snapshot'?[{id:'job',name:'empty.zip',state:'complete',done:0,total:0,error:null,path:null}]:undefined);render(<ProcessingPanel/>);await screen.findByText('empty.zip');expect((screen.getByRole('progressbar') as HTMLProgressElement).value).toBe(100);expect(screen.queryByText('Cancel processing')).toBeNull();});
