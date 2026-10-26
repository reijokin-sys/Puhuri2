// SPDX-License-Identifier: AGPL-3.0-or-later
// Exercise the actual Main.qml callbacks with reordered asynchronous RPC replies.
// This checks model behavior; it is not a Qt/Lomiri device rendering test.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../qml/Main.qml'), 'utf8');
function fixture() {
    const calls = [];
    const rows = [{thread:'alice@test', unread:1}];
    const shown = [];
    const page = {};
    const context = vm.createContext({
        root: {pythonReady:true, currentThread:'alice@test', currentName:'Alice',
            conversationPageItem:page, conversationRefreshSerial:0, messageRefreshSerial:0,
            encryptedThreads:{'alice@test':false}, encryptionFor:()=>false,
            refreshPrivacy:()=>{}, loadDraft:()=>{}, applyError:r=>r && r.ok===false},
        pageStack:{currentPage:page, push:()=>page}, conversationPage:{},
        Qt:{application:{state:1}, ApplicationActive:1},
        conversationModel:{clear:()=>rows.splice(0), append:r=>rows.push(r)},
        messageModel:shown,
        Messages:{apply:(_, messages)=>shown.splice(0,shown.length,...messages)},
        py:{call:(command,args,callback)=>calls.push({command,args,callback})},
        Object
    });
    for (const name of ['refreshConversations','openConversation','refreshMessages','markVisibleConversationRead']) {
        const match = source.match(new RegExp('    function '+name+'\\([^]*?\\n    }'));
        if (match) {
            vm.runInContext(match[0],context);
            context.root[name]=context[name];
        }
    }
    function take(command) {
        const i=calls.findIndex(c=>c.command===command);
        assert.notEqual(i,-1,'missing RPC '+command);
        return calls.splice(i,1)[0];
    }
    return {context, calls, rows, shown, take};
}
test('opening and reading a conversation refreshes the badge after successful persistence',()=>{
    const f=fixture();
    f.context.root.openConversation('alice@test','Alice',false);
    f.take('puhuri.open_conversation').callback({ok:true,result:{messages:[{id:'a'}]}});
    assert.equal(f.rows[0].unread,1,'no optimistic clear before markRead succeeds');
    f.take('puhuri.mark_read').callback({ok:true,result:{ok:true}});
    f.take('puhuri.conversations').callback({ok:true,result:[{id:'alice@test',unread:0}]});
    assert.equal(f.rows[0].unread,0);
});
test('an older conversation-list reply cannot restore a badge after the read refresh',()=>{
    const f=fixture();
    f.context.root.refreshConversations();const old=f.take('puhuri.conversations');
    f.context.root.refreshConversations();const fresh=f.take('puhuri.conversations');
    fresh.callback({ok:true,result:[{id:'alice@test',unread:0}]});
    old.callback({ok:true,result:[{id:'alice@test',unread:1}]});
    assert.equal(f.rows[0].unread,0);
});
test('a new message displayed in the active conversation is marked read',()=>{
    const f=fixture();f.context.root.refreshMessages();
    f.take('puhuri.messages').callback({ok:true,result:[{id:'new-message'}]});
    assert.equal(f.shown[0].id,'new-message');
    const read=f.take('puhuri.mark_read');assert.equal(read.args[0],'alice@test');
    read.callback({ok:true,result:{ok:true}});
    assert.ok(f.calls.some(c=>c.command==='puhuri.conversations'));
});
test('messages loaded while the app or conversation is hidden remain unread',()=>{
    for (const hide of [f=>f.context.Qt.application.state=0, f=>f.context.pageStack.currentPage={}, f=>{f.context.pageStack.currentPage=null;f.context.root.conversationPageItem=null;}]) {
        const f=fixture();hide(f);f.context.root.refreshMessages();
        f.take('puhuri.messages').callback({ok:true,result:[{id:'new-message'}]});
        assert.ok(!f.calls.some(c=>c.command==='puhuri.mark_read'));
    }
});
test('a reply for the previous thread cannot replace the visible messages or mark them read',()=>{
    const f=fixture();f.shown.push({id:'bob-message'});
    f.context.root.refreshMessages();const old=f.take('puhuri.messages');
    f.context.root.currentThread='bob@test';
    old.callback({ok:true,result:[{id:'alice-message'}]});
    assert.equal(f.shown[0].id,'bob-message');
    assert.ok(!f.calls.some(c=>c.command==='puhuri.mark_read'));
});
test('an older message-list reply cannot replace or mark read a newer snapshot',()=>{
    const f=fixture();f.context.root.refreshMessages();const old=f.take('puhuri.messages');
    f.context.root.refreshMessages();const fresh=f.take('puhuri.messages');
    fresh.callback({ok:true,result:[{id:'newest'}]});
    old.callback({ok:true,result:[{id:'older'}]});
    assert.equal(f.shown[0].id,'newest');
    assert.equal(f.calls.filter(c=>c.command==='puhuri.mark_read').length,1);
});
test('a failed read operation does not clear the badge or claim successful persistence',()=>{
    const f=fixture();f.context.root.openConversation('alice@test','Alice',false);
    f.take('puhuri.open_conversation').callback({ok:true,result:{messages:[{id:'a'}]}});
    f.take('puhuri.mark_read').callback({ok:false,error:{message:'fixture failure'}});
    assert.equal(f.rows[0].unread,1);
    assert.ok(!f.calls.some(c=>c.command==='puhuri.conversations'));
});
