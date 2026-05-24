
TODOS
- Make each node more indentifiable
- Add cloning
- Add attacking / Attacking should have an advantage 

## Conclusions 1/30 1
- Actions should have animation lengths
	- Photosythensis = 1 tick
	- Mytosis = 10 ticks
	- Attack = 1 tick
	- Consume = 10 ticks
- How do we delay animations on server?
- How do we show actions on web? Actors need to store current action on interface (will help with delay)
- What are the rounds of actions
	- Round 1. Move
	- Round 2. Action
	- Round 3. Result death and birth

## Conclusions 1/30 2
- Animations are terrible for machine learning
	- Creatures will need a way higher level of reasoning on a multi-tick level
- UI updates
	- General cleanup
	- Allow zooming
	- Differentiate actors
	- Add animations for actors
- Start neural network


Neural network initial

UI refactor
- We need to start placing actors on screen instead of grid
   - This will remove grid lines
   - This will allow animations 
   - How do we allow zooming with this?
      - Need a viewport controller
- Need to style inputs and header a bit
- Need to refactor actor style 
   - Circle. Each bar should radially decrease
   - Can assign colors to border hashed on array
   - Put ID small in center

Future UI
- Need to support backtracking and history 
- Should show actor details popup on click

Viewport controller
- Capture mouse scroll up / down
- Middle click should allow scrolling

Neural network
- Need a visualizer
- Need to define inputs
- Need to define outputs