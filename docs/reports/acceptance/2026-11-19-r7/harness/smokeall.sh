cd /tmp/claude-1000/r7/harness
bash smoke.sh r7mw2 '{"host":true}'
rm -rf /tmp/claude-1000/r7/run/r7mw2
bash smoke.sh r7ww '{"host":true,"a":["windows","Mac","#7aa7e6"],"b":["windows","PC","#a99be8"]}'
rm -rf /tmp/claude-1000/r7/run/r7ww
bash smoke.sh r7mm '{"host":true,"a":["macos","Mac","#7aa7e6"],"b":["macos","PC","#a99be8"]}'
rm -rf /tmp/claude-1000/r7/run/r7mm
echo ALL_SMOKE_DONE
