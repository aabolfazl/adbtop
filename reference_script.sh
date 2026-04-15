#!/bin/bash

# Usage: ./monitor_power.sh [package] [duration_min] [output.csv] [interval_sec] [lock_after]
# Example: ./monitor_power.sh com.example.app 60 output.csv 2 0

# Colors (defined early for usage message)
R='\033[0;31m' G='\033[0;32m' Y='\033[1;33m' C='\033[0;36m' M='\033[0;35m'
B='\033[1m' D='\033[2m' N='\033[0m' W='\033[0;37m' BG='\033[44m'

# Show usage
show_usage() {
    echo -e "${B}Usage:${N} $0 [package] [duration] [output] [interval] [lock]"
    echo ""
    echo -e "${B}Arguments:${N}"
    echo -e "  ${C}package${N}   - App package name (e.g., com.example.app)"
    echo -e "              If not provided, shows interactive selector"
    echo -e "  ${C}duration${N}  - Duration in minutes (default: 60)"
    echo -e "  ${C}output${N}    - Output CSV file (default: power_TIMESTAMP.csv)"
    echo -e "  ${C}interval${N}  - Sample interval in seconds (default: 10)"
    echo -e "  ${C}lock${N}      - Lock device after completion: 0/1 (default: 0)"
    echo ""
    echo -e "${B}Examples:${N}"
    echo -e "  $0                                    ${D}# Interactive mode${N}"
    echo -e "  $0 com.example.app                    ${D}# Monitor app for 60min${N}"
    echo -e "  $0 com.example.app 30                 ${D}# Monitor for 30min${N}"
    echo -e "  $0 com.example.app 60 output.csv 2    ${D}# 2sec interval${N}"
    echo ""
}

# Validate arguments
validate_args() {
    # If first arg is a number, user probably used old format
    if [[ "$1" =~ ^[0-9]+$ ]]; then
        echo -e "${R}Error:${N} First argument looks like a number, not a package name."
        echo -e "${Y}The argument order has changed. Package name is now first.${N}"
        echo ""
        show_usage
        exit 1
    fi

    # If first arg is provided but doesn't contain a dot (not a package name)
    if [ -n "$1" ] && [[ ! "$1" =~ \. ]]; then
        echo -e "${R}Error:${N} '$1' doesn't look like a valid package name."
        echo -e "${Y}Package names should contain dots (e.g., com.example.app)${N}"
        echo ""
        show_usage
        exit 1
    fi

    # Validate duration is a number
    if [ -n "$2" ] && [[ ! "$2" =~ ^[0-9]+$ ]]; then
        echo -e "${R}Error:${N} Duration must be a number (minutes). Got: '$2'"
        echo ""
        show_usage
        exit 1
    fi

    # Validate interval is a number
    if [ -n "$4" ] && [[ ! "$4" =~ ^[0-9]+$ ]]; then
        echo -e "${R}Error:${N} Interval must be a number (seconds). Got: '$4'"
        echo ""
        show_usage
        exit 1
    fi

    # Validate lock_after is 0 or 1
    if [ -n "$5" ] && [[ ! "$5" =~ ^[01]$ ]]; then
        echo -e "${R}Error:${N} Lock must be 0 or 1. Got: '$5'"
        echo ""
        show_usage
        exit 1
    fi
}

# Check for help flag
if [[ "$1" == "-h" ]] || [[ "$1" == "--help" ]]; then
    show_usage
    exit 0
fi

# Validate arguments
validate_args "$@"

PACKAGE="${1:-}"
DURATION=${2:-60}
OUTPUT=${3:-"power_$(date +%Y%m%d_%H%M%S).csv"}
INTERVAL=${4:-10}
LOCK_AFTER=${5:-0}

# Temperature history for charts
declare -a TEMP_AP_HISTORY TEMP_CP_HISTORY TEMP_PA_HISTORY TEMP_SKIN_HISTORY TEMP_BAT_HISTORY CPU_HISTORY
MAX_HISTORY=60

# Sparkline characters (low to high)
SPARK_CHARS=("▁" "▂" "▃" "▄" "▅" "▆" "▇" "█")

# Screen buffer for flicker-free rendering
SCREEN_BUF=""

# Get terminal dimensions
dims() {
    COLS=$(tput cols 2>/dev/null || echo 80)
    ROWS=$(tput lines 2>/dev/null || echo 24)
    IW=$((COLS - 2))
}

# Clear buffer
buf_clear() {
    SCREEN_BUF=""
}

# Add to buffer
buf_add() {
    SCREEN_BUF+="$1"
}

# Flush buffer to screen (move cursor to top, print all at once)
buf_flush() {
    printf '\033[H'  # Move cursor to top-left
    printf '%b' "$SCREEN_BUF"
    # Clear remaining lines
    printf '\033[J'  # Clear from cursor to end of screen
}

# Draw horizontal line (buffered)
hline() {
    local left="$1" right="$2" char="${3:-─}"
    local line="${C}${left}"
    for ((x=0; x<IW; x++)); do line+="$char"; done
    line+="${right}${N}\n"
    buf_add "$line"
}

# Print row with content, padded to fill width (buffered)
row() {
    local content="$1"
    local plain=$(echo -e "$content" | sed 's/\x1b\[[0-9;]*m//g')
    local len=${#plain}
    local pad=$((IW - len))
    [ $pad -lt 0 ] && pad=0
    local line="${C}│${N}${content}"
    line+="$(printf '%*s' $pad '')"
    line+="${C}│${N}\n"
    buf_add "$line"
}

# Empty row (buffered)
empty_row() {
    local line="${C}│${N}"
    line+="$(printf '%*s' $IW '')"
    line+="${C}│${N}\n"
    buf_add "$line"
}

# Build progress bar
mkbar() {
    local val=$1 max=$2 width=$3
    local filled=$((val * width / max))
    [ $filled -gt $width ] && filled=$width
    [ $filled -lt 0 ] && filled=0
    local empty=$((width - filled))
    local bar=""
    for ((x=0; x<filled; x++)); do bar+="█"; done
    for ((x=0; x<empty; x++)); do bar+="░"; done
    echo "$bar"
}

# Build sparkline from history array (compatible with bash 3.x)
# Usage: sparkline "ARRAY_NAME" width min max
sparkline() {
    local arr_name=$1
    local width=$2
    local min_val=${3:-20}
    local max_val=${4:-60}
    local result=""

    # Get array size using eval
    eval "local count=\${#${arr_name}[@]}"
    local start=$((count > width ? count - width : 0))

    for ((i=start; i<count; i++)); do
        eval "local val=\${${arr_name}[$i]%.*}"
        [ -z "$val" ] && val=$min_val
        local range=$((max_val - min_val))
        [ $range -le 0 ] && range=1
        local idx=$(( (val - min_val) * 7 / range ))
        [ $idx -lt 0 ] && idx=0
        [ $idx -gt 7 ] && idx=7
        result+="${SPARK_CHARS[$idx]}"
    done

    # Pad with spaces if not enough history
    local pad=$((width - (count - start)))
    [ $pad -gt 0 ] && result="$(printf '%*s' $pad '')$result"

    echo "$result"
}

# Color by percentage
colp() { [ $1 -gt 80 ] && echo -n "$R" || { [ $1 -gt 50 ] && echo -n "$Y" || echo -n "$G"; }; }
# Color by temperature
colt() { local t=${1%.*}; [ -z "$t" ] && t=0; [ $t -gt 45 ] && echo -n "$R" || { [ $t -gt 38 ] && echo -n "$Y" || echo -n "$G"; }; }
# Color by frequency
colf() { [ $1 -gt 2000 ] && echo -n "$R" || { [ $1 -gt 1000 ] && echo -n "$Y" || echo -n "$G"; }; }

# Check and select device with arrow keys
select_device() {
    local devices=($(adb devices | grep -v "List" | grep "device$" | awk '{print $1}'))
    local count=${#devices[@]}

    if [ $count -eq 0 ]; then
        echo -e "${R}No device connected${N}"
        exit 1
    elif [ $count -eq 1 ]; then
        DEVICE="${devices[0]}"
        local model=$(adb -s "$DEVICE" shell getprop ro.product.model 2>/dev/null | tr -d '\r')
        echo -e "${G}Using device:${N} $DEVICE ($model)"
    else
        # Get device info
        local models=()
        local brands=()
        for serial in "${devices[@]}"; do
            local m=$(adb -s "$serial" shell getprop ro.product.model 2>/dev/null | tr -d '\r\n ')
            local b=$(adb -s "$serial" shell getprop ro.product.brand 2>/dev/null | tr -d '\r\n ')
            models+=("$m")
            brands+=("$b")
        done

        echo -e "${B}Select device (↑/↓ arrows, Enter to select):${N}"
        echo ""

        local selected=0
        local key=""

        # Hide cursor
        printf '\033[?25l'

        # Draw menu function
        draw_menu() {
            # Move cursor up to redraw
            [ $1 -eq 1 ] && printf "\033[${count}A"
            for i in "${!devices[@]}"; do
                if [ $i -eq $selected ]; then
                    printf "  ${C}›${N} ${G}${devices[$i]}${N} - ${B}${brands[$i]} ${models[$i]}${N}\033[K\n"
                else
                    printf "    ${D}${devices[$i]} - ${brands[$i]} ${models[$i]}${N}\033[K\n"
                fi
            done
        }

        # Initial draw
        draw_menu 0

        # Read arrow keys
        while true; do
            read -rsn1 key
            if [[ $key == $'\x1b' ]]; then
                read -rsn2 key
                case $key in
                    '[A') # Up arrow
                        [ $selected -gt 0 ] && ((selected--))
                        draw_menu 1
                        ;;
                    '[B') # Down arrow
                        [ $selected -lt $((count-1)) ] && ((selected++))
                        draw_menu 1
                        ;;
                esac
            elif [[ $key == "" ]]; then
                # Enter pressed
                break
            fi
        done

        # Show cursor
        printf '\033[?25h'

        DEVICE="${devices[$selected]}"
        echo ""
        echo -e "${G}Selected:${N} $DEVICE (${brands[$selected]} ${models[$selected]})"
    fi
    export ANDROID_SERIAL="$DEVICE"
    echo ""
}

select_device

# Select package if not provided
select_package() {
    if [ -n "$PACKAGE" ]; then
        echo -e "${G}Package:${N} $PACKAGE"
        echo ""
        return
    fi

    echo -e "${B}Fetching running apps...${N}"

    # Get list of running 3rd party apps
    local apps=($(adb shell "ps -A -o NAME" 2>/dev/null | grep -E '^\w+\.\w+' | grep -v '^com.android' | grep -v '^com.google' | grep -v '^com.samsung' | sort -u | tr -d '\r'))

    local count=${#apps[@]}

    if [ $count -eq 0 ]; then
        echo -e "${R}No 3rd party apps running${N}"
        echo -e "${Y}Usage: $0 <package> [duration] [output] [interval] [lock_after]${N}"
        exit 1
    fi

    echo -e "${B}Select app (↑/↓ arrows, Enter to select):${N}"
    echo ""

    local selected=0
    local key=""
    local visible_start=0
    local max_visible=10

    # Hide cursor
    printf '\033[?25l'

    # Draw menu function
    draw_pkg_menu() {
        # Move cursor up to redraw
        [ $1 -eq 1 ] && printf "\033[${max_visible}A"

        # Calculate visible range
        if [ $selected -lt $visible_start ]; then
            visible_start=$selected
        elif [ $selected -ge $((visible_start + max_visible)) ]; then
            visible_start=$((selected - max_visible + 1))
        fi

        local end=$((visible_start + max_visible))
        [ $end -gt $count ] && end=$count

        for ((i=visible_start; i<end; i++)); do
            if [ $i -eq $selected ]; then
                printf "  ${C}›${N} ${G}${apps[$i]}${N}\033[K\n"
            else
                printf "    ${D}${apps[$i]}${N}\033[K\n"
            fi
        done

        # Pad remaining lines if fewer than max_visible
        for ((i=end; i<visible_start+max_visible; i++)); do
            printf "\033[K\n"
        done
    }

    # Initial draw
    draw_pkg_menu 0

    # Read arrow keys
    while true; do
        read -rsn1 key
        if [[ $key == $'\x1b' ]]; then
            read -rsn2 key
            case $key in
                '[A') # Up arrow
                    [ $selected -gt 0 ] && ((selected--))
                    draw_pkg_menu 1
                    ;;
                '[B') # Down arrow
                    [ $selected -lt $((count-1)) ] && ((selected++))
                    draw_pkg_menu 1
                    ;;
            esac
        elif [[ $key == "" ]]; then
            # Enter pressed
            break
        fi
    done

    # Show cursor
    printf '\033[?25h'

    PACKAGE="${apps[$selected]}"
    echo ""
    echo -e "${G}Selected:${N} $PACKAGE"
    echo ""
}

select_package

# Hide cursor
printf '\033[?25l'

# Cleanup function
cleanup() {
    stty sane 2>/dev/null
    printf '\033[?25h'  # Show cursor
    echo ""
    exit 0
}
trap cleanup EXIT INT TERM HUP

# Disable echo but keep signals working
stty -echo 2>/dev/null

# Get core count
NUM_CORES=$(adb shell cat /proc/cpuinfo 2>/dev/null | grep -c processor)
[ -z "$NUM_CORES" ] || [ "$NUM_CORES" -eq 0 ] && NUM_CORES=8

# CPU_TEMPS array will be populated by get_cpu_temps
CPU_TEMPS=()

# Get per-CPU thermal zones
get_cpu_temps() {
    local temps=""
    # Try to get CPU thermal zones
    local tz_data=$(adb shell "for f in /sys/class/thermal/thermal_zone*/type; do echo \$(cat \$f 2>/dev/null):\$(cat \${f%type}temp 2>/dev/null); done" 2>/dev/null)

    for ((c=0; c<NUM_CORES; c++)); do
        # Try different naming patterns for CPU temps
        local temp=$(echo "$tz_data" | grep -iE "(cpu${c}|cpu-${c}-|big${c}|little${c}|cluster${c})" | head -1 | cut -d: -f2)
        if [ -n "$temp" ] && [ "$temp" -gt 0 ] 2>/dev/null; then
            CPU_TEMPS[$c]=$(echo "scale=1; $temp / 1000" | bc 2>/dev/null || echo "0")
        else
            CPU_TEMPS[$c]=""
        fi
    done

    # If no per-CPU temps, try cluster temps
    if [ -z "${CPU_TEMPS[0]}" ]; then
        local big_temp=$(echo "$tz_data" | grep -iE "(big|cluster1|cpu-big)" | head -1 | cut -d: -f2)
        local little_temp=$(echo "$tz_data" | grep -iE "(little|cluster0|cpu-little)" | head -1 | cut -d: -f2)

        if [ -n "$big_temp" ] && [ "$big_temp" -gt 0 ] 2>/dev/null; then
            big_temp=$(echo "scale=1; $big_temp / 1000" | bc 2>/dev/null || echo "0")
            # Assign to big cores (usually last 4)
            for ((c=NUM_CORES/2; c<NUM_CORES; c++)); do
                CPU_TEMPS[$c]=$big_temp
            done
        fi
        if [ -n "$little_temp" ] && [ "$little_temp" -gt 0 ] 2>/dev/null; then
            little_temp=$(echo "scale=1; $little_temp / 1000" | bc 2>/dev/null || echo "0")
            # Assign to little cores (usually first 4)
            for ((c=0; c<NUM_CORES/2; c++)); do
                CPU_TEMPS[$c]=$little_temp
            done
        fi
    fi
}

# Previous CPU stats
declare -a PREV_TOTAL PREV_IDLE
for ((c=0; c<NUM_CORES; c++)); do PREV_TOTAL[$c]=0; PREV_IDLE[$c]=0; done

dims
printf '\033[H\033[J'  # Clear screen
buf_clear
hline "┌" "┐"
row " ${B}POWER MONITOR${N} - $NUM_CORES cores"
hline "└" "┘"
buf_flush
echo -e "${D}Package:${N} $PACKAGE"
echo -e "${D}Duration:${N} ${DURATION}m  ${D}Interval:${N} ${INTERVAL}s"
echo ""
echo -e "${Y}>${N} Disabling charging..."
adb shell dumpsys battery unplug >/dev/null 2>&1
adb shell dumpsys battery set status 3 >/dev/null 2>&1
adb shell dumpsys batterystats --reset >/dev/null 2>&1
echo -e "${Y}>${N} Keeping screen on..."
ORIG_TIMEOUT=$(adb shell settings get system screen_off_timeout 2>/dev/null)
adb shell settings put system screen_off_timeout 3600000 >/dev/null 2>&1
adb shell input keyevent 82 >/dev/null 2>&1
echo -e "${G}>${N} Ready"
sleep 1

# CSV header
echo "ts,bat,app_cpu,ap,cp,pa,bat_t,usb,skin" > "$OUTPUT"

START=$(date +%s)
END=$((START + DURATION * 60))
CNT=0 SUM_CPU=0 PEAK_CPU=0 PEAK_AP=0 PEAK_SKIN=0

while [ $(date +%s) -lt $END ]; do
    TS=$(date "+%H:%M:%S")
    dims

    # Calculate layout based on terminal size
    CHART_W=$((IW - 20))
    [ $CHART_W -lt 20 ] && CHART_W=20
    [ $CHART_W -gt $MAX_HISTORY ] && CHART_W=$MAX_HISTORY
    # Bar width: IW - (core label + brackets + pct + freq + temp + spacing)
    # " 0 [" + "] " + "100%" + " 1234MHz" + "  --  " = ~28 chars
    BAR_W=$((IW - 30))
    [ $BAR_W -lt 10 ] && BAR_W=10

    # How many thread rows we can show (always 10)
    THREAD_ROWS=10

    # === BATCH ADB CALLS FOR SPEED ===
    # Single adb call for: battery, cpu stats, frequencies, thermalservice
    BATCH_DATA=$(adb shell "
        echo '===BATTERY==='
        dumpsys battery | grep 'level:'
        echo '===STAT==='
        cat /proc/stat
        echo '===FREQ==='
        for c in \$(seq 0 $((NUM_CORES-1))); do
            cat /sys/devices/system/cpu/cpu\${c}/cpufreq/scaling_cur_freq 2>/dev/null || echo 0
        done
        echo '===THERMAL==='
        dumpsys thermalservice
    " 2>/dev/null)

    # Parse battery
    BAT=$(echo "$BATCH_DATA" | sed -n '/===BATTERY===/,/===STAT===/p' | grep -o '[0-9]*' | head -1)
    [ -z "$BAT" ] && BAT=0

    # Parse stat
    STAT=$(echo "$BATCH_DATA" | sed -n '/===STAT===/,/===FREQ===/p')

    # Parse frequencies
    FREQ_DATA=$(echo "$BATCH_DATA" | sed -n '/===FREQ===/,/===THERMAL===/p' | grep -v '===' | tr -d '\r')

    # Parse thermal
    TH=$(echo "$BATCH_DATA" | sed -n '/===THERMAL===/,$p')

    # Get app CPU and threads in parallel with main process
    TOP_DATA=$(adb shell top -n 1 -b -H 2>/dev/null)
    APP_CPU=$(echo "$TOP_DATA" | grep "$PACKAGE" | head -1 | awk '{print $9}' | tr -cd '0-9.')
    [ -z "$APP_CPU" ] && APP_CPU="0"
    APP_CPU_I=${APP_CPU%.*}
    [ -z "$APP_CPU_I" ] && APP_CPU_I=0

    # Threads from same top call
    THREADS=$(echo "$TOP_DATA" | grep "$PACKAGE" | head -15)

    # Per-core stats
    declare -a CORE_PCT CORE_MHZ
    freq_idx=0

    for ((c=0; c<NUM_CORES; c++)); do
        LINE=$(echo "$STAT" | grep "^cpu$c ")
        read -r _ user nice sys idle iow irq sirq _ <<< "$LINE"
        total=$((user + nice + sys + idle + iow + irq + sirq))

        if [ ${PREV_TOTAL[$c]} -gt 0 ]; then
            dt=$((total - PREV_TOTAL[$c]))
            di=$((idle - PREV_IDLE[$c]))
            [ $dt -gt 0 ] && pct=$(( (dt - di) * 100 / dt )) || pct=0
        else
            pct=0
        fi
        PREV_TOTAL[$c]=$total
        PREV_IDLE[$c]=$idle
        CORE_PCT[$c]=$pct

        freq=$(echo "$FREQ_DATA" | sed -n "$((c+1))p" | tr -cd '0-9')
        [ -z "$freq" ] && freq=0
        CORE_MHZ[$c]=$((freq / 1000))
    done

    # Get per-CPU temperatures
    get_cpu_temps

    # System temperatures (from cached TH data)
    T_AP=$(echo "$TH" | grep "mName=AP," | head -1 | grep -oE "mValue=[0-9.]+" | grep -oE "[0-9.]+")
    T_CP=$(echo "$TH" | grep "mName=CP," | head -1 | grep -oE "mValue=[0-9.]+" | grep -oE "[0-9.]+")
    T_PA=$(echo "$TH" | grep "mName=PA," | head -1 | grep -oE "mValue=[0-9.]+" | grep -oE "[0-9.]+")
    T_BAT=$(echo "$TH" | grep "mName=BAT," | head -1 | grep -oE "mValue=[0-9.]+" | grep -oE "[0-9.]+")
    T_USB=$(echo "$TH" | grep "mName=USB," | head -1 | grep -oE "mValue=[0-9.]+" | grep -oE "[0-9.]+")
    T_SKIN=$(echo "$TH" | grep "mName=SKIN," | head -1 | grep -oE "mValue=[0-9.]+" | grep -oE "[0-9.]+")
    [ -z "$T_AP" ] && T_AP=0; [ -z "$T_CP" ] && T_CP=0; [ -z "$T_PA" ] && T_PA=0
    [ -z "$T_BAT" ] && T_BAT=0; [ -z "$T_USB" ] && T_USB=0; [ -z "$T_SKIN" ] && T_SKIN=0

    # Add to history arrays
    TEMP_AP_HISTORY+=("${T_AP%.*}")
    TEMP_CP_HISTORY+=("${T_CP%.*}")
    TEMP_PA_HISTORY+=("${T_PA%.*}")
    TEMP_SKIN_HISTORY+=("${T_SKIN%.*}")
    TEMP_BAT_HISTORY+=("${T_BAT%.*}")
    CPU_HISTORY+=("$APP_CPU_I")

    # Trim history to max size
    [ ${#TEMP_AP_HISTORY[@]} -gt $MAX_HISTORY ] && TEMP_AP_HISTORY=("${TEMP_AP_HISTORY[@]:1}")
    [ ${#TEMP_CP_HISTORY[@]} -gt $MAX_HISTORY ] && TEMP_CP_HISTORY=("${TEMP_CP_HISTORY[@]:1}")
    [ ${#TEMP_PA_HISTORY[@]} -gt $MAX_HISTORY ] && TEMP_PA_HISTORY=("${TEMP_PA_HISTORY[@]:1}")
    [ ${#TEMP_SKIN_HISTORY[@]} -gt $MAX_HISTORY ] && TEMP_SKIN_HISTORY=("${TEMP_SKIN_HISTORY[@]:1}")
    [ ${#TEMP_BAT_HISTORY[@]} -gt $MAX_HISTORY ] && TEMP_BAT_HISTORY=("${TEMP_BAT_HISTORY[@]:1}")
    [ ${#CPU_HISTORY[@]} -gt $MAX_HISTORY ] && CPU_HISTORY=("${CPU_HISTORY[@]:1}")

    AP_I=$(echo "$T_AP * 10" | bc 2>/dev/null | cut -d. -f1)
    [ -z "$AP_I" ] && AP_I=0
    SKIN_I=$(echo "$T_SKIN * 10" | bc 2>/dev/null | cut -d. -f1)
    [ -z "$SKIN_I" ] && SKIN_I=0

    # === Calculate per-core app CPU usage ===
    # Read threads into array
    thread_lines=()
    while IFS= read -r line; do
        [ -n "$line" ] && thread_lines+=("$line")
    done <<< "$THREADS"

    # Get ALL PIDs for core lookup
    all_pids=""
    for ((t=0; t<${#thread_lines[@]}; t++)); do
        pid=$(echo "${thread_lines[$t]}" | awk '{print $1}')
        all_pids+="$pid "
    done

    # Batch get all thread cores in one adb call
    core_data=""
    if [ -n "$all_pids" ]; then
        core_data=$(adb shell "for p in $all_pids; do echo \$p:\$(awk '{print \$39}' /proc/\$p/stat 2>/dev/null); done" 2>/dev/null)
    fi

    # Initialize per-core app CPU arrays
    for ((c=0; c<NUM_CORES; c++)); do
        APP_CORE_CPU[$c]=0
        APP_CORE_THREADS[$c]=0
    done

    # Process ALL threads to calculate per-core CPU usage
    for ((t=0; t<${#thread_lines[@]}; t++)); do
        line="${thread_lines[$t]}"
        [ -z "$line" ] && continue
        pid=$(echo "$line" | awk '{print $1}')
        cpu=$(echo "$line" | awk '{print $9}')

        # Get core from batch data
        core=$(echo "$core_data" | grep "^${pid}:" | cut -d: -f2 | tr -cd '0-9')

        if [ -n "$core" ] && [ "$core" -ge 0 ] 2>/dev/null && [ "$core" -lt $NUM_CORES ]; then
            cpu_val=${cpu%.*}
            [ -z "$cpu_val" ] && cpu_val=0
            APP_CORE_CPU[$core]=$((APP_CORE_CPU[$core] + cpu_val))
            APP_CORE_THREADS[$core]=$((APP_CORE_THREADS[$core] + 1))
        fi
    done

    # Stats
    CNT=$((CNT + 1))
    SUM_CPU=$((SUM_CPU + APP_CPU_I))
    [ $APP_CPU_I -gt $PEAK_CPU ] && PEAK_CPU=$APP_CPU_I
    [ $AP_I -gt $PEAK_AP ] && PEAK_AP=$AP_I
    [ $SKIN_I -gt $PEAK_SKIN ] && PEAK_SKIN=$SKIN_I

    # CSV
    echo "$TS,$BAT,$APP_CPU,$T_AP,$T_CP,$T_PA,$T_BAT,$T_USB,$T_SKIN" >> "$OUTPUT"

    EL=$(( ($(date +%s) - START) / 60 ))
    EL_S=$(( ($(date +%s) - START) % 60 ))
    RM=$((DURATION - EL))
    AVG=$((SUM_CPU / CNT))
    PEAK_AP_C=$(echo "scale=1;$PEAK_AP/10" | bc 2>/dev/null || echo "0")
    PEAK_SKIN_C=$(echo "scale=1;$PEAK_SKIN/10" | bc 2>/dev/null || echo "0")

    # ===================== RENDER =====================
    buf_clear

    # Header
    hline "┌" "┐"
    row " ${B}⚡ POWER${N} $TS ${D}│${N} ${G}${EL}m${EL_S}s${N}/${Y}${RM}m${N} ${D}│${N} #${CNT} ${D}│${N} ${G}${BAT}%${N} ${D}│${N} ${C}$PACKAGE${N}"
    hline "╞" "╡" "═"

    # CPU Cores section
    row " ${B}CPU CORES${N}"
    hline "├" "┤"

    for ((c=0; c<NUM_CORES; c++)); do
        pct=${CORE_PCT[$c]}
        mhz=${CORE_MHZ[$c]}
        temp=${CPU_TEMPS[$c]}
        pc=$(colp $pct)
        fc=$(colf $mhz)
        bar=$(mkbar $pct 100 $BAR_W)

        # Format temperature if available
        if [ -n "$temp" ] && [ "$temp" != "0" ]; then
            tc=$(colt $temp)
            temp_str="${tc}$(printf '%4.1f' $temp)°C${N}"
        else
            temp_str="${D}  --  ${N}"
        fi

        row " ${pc}${B}$c${N} [${pc}${bar}${N}] ${pc}$(printf '%3d' $pct)%${N} ${fc}$(printf '%4d' $mhz)MHz${N} $temp_str"
    done

    hline "╞" "╡" "═"

    # APP CPU PER CORE section
    row " ${B}APP CPU PER CORE${N}"
    hline "├" "┤"

    APP_BAR_W2=$((IW - 24))
    [ $APP_BAR_W2 -lt 10 ] && APP_BAR_W2=10

    for ((c=0; c<NUM_CORES; c++)); do
        app_pct=${APP_CORE_CPU[$c]:-0}
        app_threads=${APP_CORE_THREADS[$c]:-0}
        apc=$(colp $app_pct)
        app_bar=$(mkbar $app_pct 100 $APP_BAR_W2)
        row " ${apc}${B}$c${N} [${apc}${app_bar}${N}] ${apc}$(printf '%3d' $app_pct)%${N} ${D}(${app_threads} threads)${N}"
    done

    hline "╞" "╡" "═"

    # Temperature section with charts
    row " ${B}TEMPERATURES${N}"
    hline "├" "┤"

    ac=$(colt $T_AP); cc=$(colt $T_CP); pc=$(colt $T_PA); sc=$(colt $T_SKIN); bc=$(colt $T_BAT)
    row "  ${ac}${B}AP${N}:$(printf '%5.1f' $T_AP)°C  ${cc}${B}CP${N}:$(printf '%5.1f' $T_CP)°C  ${pc}${B}PA${N}:$(printf '%5.1f' $T_PA)°C  ${sc}${B}SKIN${N}:$(printf '%5.1f' $T_SKIN)°C  ${bc}${B}BAT${N}:$(printf '%5.1f' $T_BAT)°C  ${B}USB${N}:$(printf '%5.1f' $T_USB)°C"
    hline "├" "┤"

    # Temperature sparkline charts
    CHART_W2=$((IW - 22))
    [ $CHART_W2 -lt 20 ] && CHART_W2=20
    [ $CHART_W2 -gt $MAX_HISTORY ] && CHART_W2=$MAX_HISTORY

    spark_ap=$(sparkline TEMP_AP_HISTORY $CHART_W2 20 55)
    spark_cp=$(sparkline TEMP_CP_HISTORY $CHART_W2 20 55)
    spark_pa=$(sparkline TEMP_PA_HISTORY $CHART_W2 20 55)
    spark_skin=$(sparkline TEMP_SKIN_HISTORY $CHART_W2 20 55)
    spark_bat=$(sparkline TEMP_BAT_HISTORY $CHART_W2 20 45)

    row " ${R}AP  ${N} ${Y}$spark_ap${N} ${D}$(printf '%4.1f' $T_AP)°${N}"
    row " ${M}CP  ${N} ${C}$spark_cp${N} ${D}$(printf '%4.1f' $T_CP)°${N}"
    row " ${M}PA  ${N} ${C}$spark_pa${N} ${D}$(printf '%4.1f' $T_PA)°${N}"
    row " ${Y}SKIN${N} ${Y}$spark_skin${N} ${D}$(printf '%4.1f' $T_SKIN)°${N}"
    row " ${G}BAT ${N} ${G}$spark_bat${N} ${D}$(printf '%4.1f' $T_BAT)°${N}"

    hline "├" "┤"
    # CPU sparkline chart
    cpu_spark=$(sparkline CPU_HISTORY $CHART_W2 0 150)
    row " ${C}CPU ${N} ${G}$cpu_spark${N} ${D}$(printf '%5.1f' $APP_CPU)%${N}"

    hline "╞" "╡" "═"

    # Threads section
    row " ${B}TOP THREADS${N}"
    hline "├" "┤"
    row "  ${D}PID      CPU   MEM   CORE  TIME       NAME${N}"

    # Display top threads (data already calculated earlier)
    for ((t=0; t<THREAD_ROWS && t<${#thread_lines[@]}; t++)); do
        line="${thread_lines[$t]}"
        [ -z "$line" ] && continue
        pid=$(echo "$line" | awk '{print $1}')
        cpu=$(echo "$line" | awk '{print $9}')
        mem=$(echo "$line" | awk '{print $10}')
        tm=$(echo "$line" | awk '{print $11}')
        nm=$(echo "$line" | awk '{print $12}')

        # Get core from batch data using grep
        core=$(echo "$core_data" | grep "^${pid}:" | cut -d: -f2 | tr -cd '0-9')
        [ -z "$core" ] && core="-"

        cpu_i=${cpu%.*}; [ -z "$cpu_i" ] && cpu_i=0

        # Color entire row based on CPU usage
        if [ $cpu_i -gt 50 ]; then
            rc="$R"  # Red for high CPU
        elif [ $cpu_i -gt 20 ]; then
            rc="$Y"  # Yellow for medium CPU
        elif [ $cpu_i -gt 5 ]; then
            rc="$G"  # Green for low CPU
        else
            rc="$D"  # Dim for idle
        fi

        row "  ${rc}$(printf '%-8s' $pid) $(printf '%5s' $cpu)% $(printf '%4s' $mem)%  $(printf '%2s' $core)   $(printf '%-10s' $tm) $nm${N}"
    done

    hline "╞" "╡" "═"

    # Stats footer
    row " ${D}STATS${N}  Avg CPU: ${G}${AVG}%${N}  Peak CPU: ${Y}${PEAK_CPU}%${N}  Peak AP: ${R}${PEAK_AP_C}°C${N}  Peak SKIN: ${R}${PEAK_SKIN_C}°C${N}"
    hline "├" "┤"
    row " ${D}Output: $OUTPUT${N}"
    hline "└" "┘"

    # Flush buffer to screen
    buf_flush

    sleep $INTERVAL
done

# ===================== CLEANUP =====================
printf '\033[H\033[J'  # Clear screen
adb shell dumpsys battery reset >/dev/null 2>&1
[ -n "$ORIG_TIMEOUT" ] && adb shell settings put system screen_off_timeout "$ORIG_TIMEOUT" >/dev/null 2>&1

F_BAT=$(awk -F',' 'NR==2{print $2}' "$OUTPUT")
L_BAT=$(tail -1 "$OUTPUT" | awk -F',' '{print $2}')
[ -z "$F_BAT" ] && F_BAT=0; [ -z "$L_BAT" ] && L_BAT=0
DRAIN=$((F_BAT - L_BAT))
AVG=$((SUM_CPU / CNT))
PEAK_AP_C=$(echo "scale=1;$PEAK_AP/10" | bc 2>/dev/null || echo "0")
PEAK_SKIN_C=$(echo "scale=1;$PEAK_SKIN/10" | bc 2>/dev/null || echo "0")

dims
buf_clear
hline "┌" "┐"
row " ${G}${B}✓ COMPLETE${N} - Charging re-enabled, screen timeout restored"
hline "╞" "╡" "═"
row " ${B}FINAL REPORT${N}"
hline "├" "┤"
row "  Samples: ${M}$CNT${N}     Duration: ${C}${DURATION}m${N}"
row "  Avg CPU: ${G}${AVG}%${N}   Peak CPU: ${Y}${PEAK_CPU}%${N}"
row "  Peak AP: ${R}${PEAK_AP_C}°C${N}  Peak SKIN: ${R}${PEAK_SKIN_C}°C${N}"
row "  Battery: ${G}${F_BAT}%${N} → ${Y}${L_BAT}%${N} (${R}-${DRAIN}%${N} drain)"
hline "├" "┤"
row " ${D}Output: $OUTPUT${N}"
hline "└" "┘"
buf_flush

[ "$LOCK_AFTER" = "1" ] && { echo -e "${D}Locking device...${N}"; adb shell input keyevent 26; }
