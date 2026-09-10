for (var source of ['\\B','(\\B)','(?=\\B)','\\B.']) {
  for (var flags of ['du','duy']) {
    for(var at of (flags.indexOf('y')>=0 ? [0,1,2,3,4] : [0])) {
      var re=new RegExp(source,flags);re.lastIndex=at;var match=re.exec('a\ud83d\ude00b');
      console.log(JSON.stringify({source:source,flags:flags,start:at,indices:match===null?null:match.indices}));
    }
  }
}
