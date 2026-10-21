#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Read voice-file metadata on the phone; emits no paths, account data or audio.
# Run from the project: adb shell -T python3 - < tools/audit-voice-storage.py
import pathlib,json,struct,hashlib,wave,ctypes,ctypes.util
root=pathlib.Path.home()/'.local/share'
result={}
for app in ['fi.reijo.puhuri','fi.reijo.puhuri2']:
    folders={}; hashes={}
    for folder in ['recordings','attachments']:
        base=root/app/folder
        files=[p for p in base.rglob('*') if p.is_file()] if base.exists() else []
        stats={'exists':base.exists(),'files':len(files),'bytes':sum(p.stat().st_size for p in files),'audio':[]}
        for p in files:
            if p.suffix.lower() not in ['.ogg','.oga','.wav','.m4a','.opus']: continue
            size=p.stat().st_size
            item={'extension':p.suffix.lower(),'bytes':size}
            digest=hashlib.sha256()
            with p.open('rb') as f:
                header=f.read(128); digest.update(header)
                for block in iter(lambda:f.read(1024*1024),b''): digest.update(block)
            hashes.setdefault(digest.hexdigest(),[]).append((folder,size))
            if header[:4]==b'OggS':
                if b'\x01vorbis' in header:
                    off=header.index(b'\x01vorbis')
                    version,channels,rate,bmax,bnom,bmin=struct.unpack_from('<IBIiii',header,off+7)
                    item.update(codec='vorbis',channels=channels,rate=rate,nominal_bitrate=bnom)
                    granule=0
                    with p.open('rb') as f:
                        while True:
                            page=f.read(27)
                            if len(page)<27 or page[:4]!=b'OggS': break
                            g=struct.unpack_from('<Q',page,6)[0]
                            if g!=(1<<64)-1: granule=max(granule,g)
                            segments=f.read(page[26]); f.seek(sum(segments),1)
                    if rate and granule:
                        item['seconds']=round(granule/rate,3)
                        item['actual_kbit_s']=round(size*8/(granule/rate)/1000,2)
                elif b'OpusHead' in header:
                    off=header.index(b'OpusHead')
                    skip=struct.unpack_from('<H',header,off+10)[0]
                    item.update(codec='opus',channels=header[off+9],rate=48000)
                    granule=0
                    with p.open('rb') as f:
                        while True:
                            page=f.read(27)
                            if len(page)<27 or page[:4]!=b'OggS': break
                            g=struct.unpack_from('<Q',page,6)[0]
                            if g!=(1<<64)-1: granule=max(granule,g)
                            segments=f.read(page[26]); f.seek(sum(segments),1)
                    if granule>skip:
                        seconds=(granule-skip)/48000
                        item['seconds']=round(seconds,3)
                        item['actual_kbit_s']=round(size*8/seconds/1000,2)
                else: item['codec']='ogg-unknown'
            elif header[:4]==b'RIFF':
                try:
                    with wave.open(str(p)) as f:
                        item.update(codec='pcm',rate=f.getframerate(),channels=f.getnchannels(),bits=f.getsampwidth()*8,seconds=round(f.getnframes()/f.getframerate(),3))
                except Exception: item['codec']='riff-unreadable'
            stats['audio'].append(item)
        folders[folder]=stats
    duplicates=[v for v in hashes.values() if len(v)>1 and len(set(x[0] for x in v))>1]
    result[app]={'folders':folders,'cross_directory_identical_groups':len(duplicates),'duplicate_bytes':sum(sum(s for _,s in v)-v[0][1] for v in duplicates)}
try:
    gst=ctypes.CDLL(ctypes.util.find_library('gstreamer-1.0'))
    gst.gst_init.argtypes=[ctypes.c_void_p,ctypes.c_void_p]; gst.gst_init(None,None)
    gst.gst_element_factory_find.argtypes=[ctypes.c_char_p]; gst.gst_element_factory_find.restype=ctypes.c_void_p
    gst.gst_object_unref.argtypes=[ctypes.c_void_p]
    factories={}
    for name in ['opusenc','opusdec','vorbisenc','vorbisdec','oggmux']:
        obj=gst.gst_element_factory_find(name.encode()); factories[name]=bool(obj)
        if obj: gst.gst_object_unref(obj)
    result['gstreamer_factories']=factories
except Exception as e: result['gstreamer_probe_error']=str(e)
print(json.dumps(result,indent=2))

