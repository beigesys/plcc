#!/bin/sh
# Start FUXA, wait for ready, load project from JSON

node main.js &
FUXA_PID=$!

# Wait for FUXA API
for i in $(seq 1 30); do
    if node -e "require('http').get('http://localhost:1881/api/project',r=>{process.exit(r.statusCode===200?0:1)})" 2>/dev/null; then
        break
    fi
    sleep 1
done

# Load project if provided
if [ -f /project/project.json ]; then
    node -e "
const fs = require('fs');
const http = require('http');
const data = fs.readFileSync('/project/project.json');
const req = http.request({hostname:'localhost',port:1881,path:'/api/project',method:'POST',headers:{'Content-Type':'application/json','Content-Length':data.length}}, res => {
    console.log('Project loaded: ' + res.statusCode);
});
req.write(data);
req.end();
" 2>&1
fi

wait $FUXA_PID
