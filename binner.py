import sys
import time

def braille_spinning_dots(duration):
    spinner_frames = [
        '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'
    ]
    
    end_time = time.time() + duration
    while time.time() < end_time:
        for frame in spinner_frames:
            sys.stdout.write(f'\r{frame}')
            sys.stdout.flush()
            time.sleep(0.1)
    sys.stdout.write('\rDone!\n')
    sys.stdout.flush()

# Run the spinner for 5 seconds
braille_spinning_dots(5)

