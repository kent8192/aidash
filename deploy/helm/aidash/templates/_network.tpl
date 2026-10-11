{{- /*
Renders "true" when the value is an address the server's ProxyNetwork accepts
(server/src/http.rs): an IPv4 or IPv6 address, optionally with a prefix no
longer than its width and no host bits set. The server refuses anything else
at startup, so rendering must refuse it first.
*/ -}}
{{- define "aidash.proxyNetwork" -}}
{{- $parts := splitList "/" (toString .) -}}
{{- $address := first $parts -}}
{{- $octet := "(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])" -}}
{{- $v4 := printf "^%s(\\.%s){3}$" $octet $octet -}}
{{- $hex := dict "0" 0 "1" 1 "2" 2 "3" 3 "4" 4 "5" 5 "6" 6 "7" 7 "8" 8 "9" 9 "a" 10 "b" 11 "c" 12 "d" 13 "e" 14 "f" 15 -}}
{{- $valid := le (len $parts) 2 -}}
{{- $width := 0 -}}
{{- $units := list -}}
{{- if regexMatch $v4 $address -}}
{{- $width = 8 -}}
{{- range splitList "." $address -}}{{- $units = append $units (atoi .) -}}{{- end -}}
{{- else -}}
{{- $width = 16 -}}
{{- $halves := splitList "::" $address -}}
{{- $valid = and $valid (le (len $halves) 2) -}}
{{- $sides := list -}}
{{- range $side, $half := $halves -}}
{{- $values := list -}}
{{- if $half -}}
{{- $tokens := splitList ":" $half -}}
{{- range $position, $token := $tokens -}}
{{- if and (contains "." $token) (eq $side (sub (len $halves) 1)) (eq $position (sub (len $tokens) 1)) (regexMatch $v4 $token) -}}
{{- /* An embedded IPv4 tail holds the last two groups. */ -}}
{{- $bytes := splitList "." $token -}}
{{- $values = append $values (add (mul (atoi (index $bytes 0)) 256) (atoi (index $bytes 1))) -}}
{{- $values = append $values (add (mul (atoi (index $bytes 2)) 256) (atoi (index $bytes 3))) -}}
{{- else if regexMatch "^[0-9A-Fa-f]{1,4}$" $token -}}
{{- $value := 0 -}}
{{- range splitList "" (lower $token) -}}{{- $value = add (mul $value 16) (get $hex .) -}}{{- end -}}
{{- $values = append $values $value -}}
{{- else -}}
{{- $valid = false -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- $sides = append $sides $values -}}
{{- end -}}
{{- if eq (len $sides) 2 -}}
{{- /* "::" stands for at least one zero group. */ -}}
{{- $explicit := add (len (index $sides 0)) (len (index $sides 1)) -}}
{{- $valid = and $valid (le $explicit 7) -}}
{{- $units = index $sides 0 -}}
{{- range until (int (max 0 (sub 8 $explicit))) -}}{{- $units = append $units 0 -}}{{- end -}}
{{- $units = concat $units (index $sides 1) -}}
{{- else if eq (len $sides) 1 -}}
{{- $units = index $sides 0 -}}
{{- $valid = and $valid (eq (len $units) 8) -}}
{{- end -}}
{{- end -}}
{{- $total := mul $width (len $units) -}}
{{- $prefix := $total -}}
{{- if eq (len $parts) 2 -}}
{{- $raw := index $parts 1 -}}
{{- if regexMatch "^0*[0-9]{1,3}$" $raw -}}{{- $prefix = atoi $raw -}}{{- else -}}{{- $valid = false -}}{{- end -}}
{{- end -}}
{{- $valid = and $valid (gt $total 0) (le $prefix $total) -}}
{{- if $valid -}}
{{- $powers := list 1 2 4 8 16 32 64 128 256 512 1024 2048 4096 8192 16384 32768 65536 -}}
{{- range $position, $value := $units -}}
{{- $network := max 0 (min $width (sub $prefix (mul $width $position))) -}}
{{- if ne (mod $value (index $powers (int (sub $width $network)))) 0 -}}{{- $valid = false -}}{{- end -}}
{{- end -}}
{{- end -}}
{{- if $valid -}}true{{- end -}}
{{- end -}}
